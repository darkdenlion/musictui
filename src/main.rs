use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Wrap,
    },
    Frame, Terminal,
};
use std::{
    io::{self, stdout},
    process::Command,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::time::interval;

// ── Constants ──────────────────────────────────────────────────────

const APP_NAME: &str = "Music";
const POLL_INTERVAL: Duration = Duration::from_secs(2);
const TICK_RATE: Duration = Duration::from_millis(100);

// ── Themes ─────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
struct Theme {
    accent: Color,
    green: Color,
    yellow: Color,
    red: Color,
    dim: Color,
    surface: Color,
    surface_light: Color,
    text: Color,
    text_dim: Color,
    border: Color,
    highlight_bg: Color,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum ThemeName {
    Default,
    Dracula,
    Catppuccin,
    Nord,
    Gruvbox,
}

impl ThemeName {
    fn label(&self) -> &str {
        match self {
            Self::Default => "Default",
            Self::Dracula => "Dracula",
            Self::Catppuccin => "Catppuccin",
            Self::Nord => "Nord",
            Self::Gruvbox => "Gruvbox",
        }
    }

    fn next(&self) -> Self {
        match self {
            Self::Default => Self::Dracula,
            Self::Dracula => Self::Catppuccin,
            Self::Catppuccin => Self::Nord,
            Self::Nord => Self::Gruvbox,
            Self::Gruvbox => Self::Default,
        }
    }

    fn theme(&self) -> Theme {
        match self {
            Self::Default => Theme {
                accent: Color::Rgb(100, 180, 255),
                green: Color::Rgb(80, 220, 130),
                yellow: Color::Rgb(240, 200, 80),
                red: Color::Rgb(240, 90, 90),
                dim: Color::Rgb(100, 100, 115),
                surface: Color::Reset,
                surface_light: Color::Reset,
                text: Color::Rgb(220, 220, 230),
                text_dim: Color::Rgb(140, 140, 155),
                border: Color::Rgb(55, 55, 70),
                highlight_bg: Color::Rgb(60, 60, 80),
            },
            Self::Dracula => Theme {
                accent: Color::Rgb(189, 147, 249),
                green: Color::Rgb(80, 250, 123),
                yellow: Color::Rgb(241, 250, 140),
                red: Color::Rgb(255, 85, 85),
                dim: Color::Rgb(98, 114, 164),
                surface: Color::Reset,
                surface_light: Color::Reset,
                text: Color::Rgb(248, 248, 242),
                text_dim: Color::Rgb(98, 114, 164),
                border: Color::Rgb(68, 71, 90),
                highlight_bg: Color::Rgb(68, 71, 90),
            },
            Self::Catppuccin => Theme {
                accent: Color::Rgb(137, 180, 250),
                green: Color::Rgb(166, 227, 161),
                yellow: Color::Rgb(249, 226, 175),
                red: Color::Rgb(243, 139, 168),
                dim: Color::Rgb(108, 112, 134),
                surface: Color::Reset,
                surface_light: Color::Reset,
                text: Color::Rgb(205, 214, 244),
                text_dim: Color::Rgb(147, 153, 178),
                border: Color::Rgb(69, 71, 90),
                highlight_bg: Color::Rgb(49, 50, 68),
            },
            Self::Nord => Theme {
                accent: Color::Rgb(136, 192, 208),
                green: Color::Rgb(163, 190, 140),
                yellow: Color::Rgb(235, 203, 139),
                red: Color::Rgb(191, 97, 106),
                dim: Color::Rgb(76, 86, 106),
                surface: Color::Reset,
                surface_light: Color::Reset,
                text: Color::Rgb(236, 239, 244),
                text_dim: Color::Rgb(129, 161, 193),
                border: Color::Rgb(67, 76, 94),
                highlight_bg: Color::Rgb(67, 76, 94),
            },
            Self::Gruvbox => Theme {
                accent: Color::Rgb(131, 165, 152),
                green: Color::Rgb(184, 187, 38),
                yellow: Color::Rgb(250, 189, 47),
                red: Color::Rgb(251, 73, 52),
                dim: Color::Rgb(146, 131, 116),
                surface: Color::Reset,
                surface_light: Color::Reset,
                text: Color::Rgb(235, 219, 178),
                text_dim: Color::Rgb(168, 153, 132),
                border: Color::Rgb(80, 73, 69),
                highlight_bg: Color::Rgb(80, 73, 69),
            },
        }
    }
}

// ── Config ─────────────────────────────────────────────────────────

fn config_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    std::path::PathBuf::from(home)
        .join(".config")
        .join("musictui")
        .join("config")
}

fn load_config_theme() -> ThemeName {
    let path = config_path();
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return ThemeName::Default,
    };
    for line in content.lines() {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("theme=") {
            return match val.trim() {
                "Dracula" => ThemeName::Dracula,
                "Catppuccin" => ThemeName::Catppuccin,
                "Nord" => ThemeName::Nord,
                "Gruvbox" => ThemeName::Gruvbox,
                _ => ThemeName::Default,
            };
        }
    }
    ThemeName::Default
}

fn save_config_theme(theme: ThemeName) {
    let path = config_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, format!("theme={}\n", theme.label()));
}

// ── Data ───────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct PlaylistTrack {
    name: String,
    artist: String,
    duration: f64,
    index: usize, // 1-based index in the playlist
}

#[derive(Clone, Debug, PartialEq)]
enum BrowseView {
    Playlists,
    Tracks(String), // playlist name
    GlobalSearch,
    Artists,
    ArtistTracks(String), // artist name
    RecentlyPlayed,
}

#[derive(Clone, Debug)]
struct TrackInfo {
    name: String,
    artist: String,
    album: String,
    state: PlayerState,
    duration: f64,
    position: f64,
    loved: Option<bool>,
}

impl Default for TrackInfo {
    fn default() -> Self {
        Self {
            name: String::new(),
            artist: String::new(),
            album: String::new(),
            state: PlayerState::Stopped,
            duration: 0.0,
            position: 0.0,
            loved: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum PlayerState {
    Playing,
    Paused,
    Stopped,
    NotRunning,
}

impl PlayerState {
    fn from_str(s: &str) -> Self {
        match s.trim().to_uppercase().as_str() {
            "PLAYING" => Self::Playing,
            "PAUSED" => Self::Paused,
            "NOT_RUNNING" => Self::NotRunning,
            _ => Self::Stopped,
        }
    }

    fn icon(&self) -> &str {
        match self {
            Self::Playing => "▶",
            Self::Paused => "⏸",
            Self::Stopped => "⏹",
            Self::NotRunning => "○",
        }
    }

    fn label(&self) -> &str {
        match self {
            Self::Playing => "Playing",
            Self::Paused => "Paused",
            Self::Stopped => "Stopped",
            Self::NotRunning => "Not Running",
        }
    }

    fn color(&self, theme: &Theme) -> Color {
        match self {
            Self::Playing => theme.green,
            Self::Paused => theme.yellow,
            _ => theme.dim,
        }
    }
}

#[derive(Clone, Debug)]
struct AppState {
    track: TrackInfo,
    playlists: Vec<String>,
    playlist_tracks: Vec<PlaylistTrack>,
    up_next_name: String,
    up_next_artist: String,
    queue: Vec<(String, String)>, // (name, artist) of upcoming tracks
    shuffle: Option<bool>,
    repeat_mode: Option<String>,
    volume: i32,
    current_playlist: String,
    last_position_time: Instant,
    pre_mute_vol: i32,
    status: String,
    status_time: Instant,
    dirty: bool,
    artists: Vec<String>,
    artist_tracks: Vec<PlaylistTrack>,
    recent_tracks: Vec<(String, String)>, // (name, artist) — most recent first
    last_track_name: String,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            track: TrackInfo::default(),
            playlists: Vec::new(),
            playlist_tracks: Vec::new(),
            up_next_name: String::new(),
            up_next_artist: String::new(),
            queue: Vec::new(),
            shuffle: None,
            repeat_mode: None,
            volume: -1,
            current_playlist: String::new(),
            last_position_time: Instant::now(),
            pre_mute_vol: 50,
            status: "Loading...".into(),
            status_time: Instant::now(),
            dirty: false,
            artists: Vec::new(),
            artist_tracks: Vec::new(),
            recent_tracks: Vec::new(),
            last_track_name: String::new(),
        }
    }
}

enum InputMode {
    Normal,
    Search,
}

struct App {
    state: Arc<Mutex<AppState>>,
    list_state: ListState,
    track_list_state: ListState,
    browse_view: BrowseView,
    input_mode: InputMode,
    search_query: String,
    filtered_indices: Vec<usize>,
    filtered_track_indices: Vec<usize>,
    show_help: bool,
    should_quit: bool,
    mini_mode: bool,
    theme_name: ThemeName,
    theme: Theme,
    progress_bar_area: Option<Rect>,
    show_airplay: bool,
    airplay_devices: Vec<(String, bool)>, // (name, selected)
    airplay_list_state: ListState,
    global_search_results: Vec<PlaylistTrack>,
    global_search_state: ListState,
    global_search_query: String,
    artist_list_state: ListState,
    artist_track_list_state: ListState,
    filtered_artist_indices: Vec<usize>,
    filtered_artist_track_indices: Vec<usize>,
    show_add_to_playlist: bool,
    add_to_playlist_state: ListState,
    recent_list_state: ListState,
}

impl App {
    fn new(state: Arc<Mutex<AppState>>) -> Self {
        let theme_name = load_config_theme();
        let mut app = Self {
            state,
            list_state: ListState::default(),
            track_list_state: ListState::default(),
            browse_view: BrowseView::Playlists,
            input_mode: InputMode::Normal,
            search_query: String::new(),
            filtered_indices: Vec::new(),
            filtered_track_indices: Vec::new(),
            show_help: false,
            should_quit: false,
            mini_mode: false,
            theme_name,
            theme: theme_name.theme(),
            progress_bar_area: None,
            show_airplay: false,
            airplay_devices: Vec::new(),
            airplay_list_state: ListState::default(),
            global_search_results: Vec::new(),
            global_search_state: ListState::default(),
            global_search_query: String::new(),
            artist_list_state: ListState::default(),
            artist_track_list_state: ListState::default(),
            filtered_artist_indices: Vec::new(),
            filtered_artist_track_indices: Vec::new(),
            show_add_to_playlist: false,
            add_to_playlist_state: ListState::default(),
            recent_list_state: ListState::default(),
        };
        app.update_filter();
        app
    }

    fn update_artist_filter(&mut self) {
        let st = self.state.lock().unwrap();
        let query = &self.search_query;
        if query.is_empty() {
            self.filtered_artist_indices = (0..st.artists.len()).collect();
        } else {
            let mut scored: Vec<(usize, i32)> = st
                .artists
                .iter()
                .enumerate()
                .filter_map(|(i, name)| fuzzy_score(query, name).map(|s| (i, s)))
                .collect();
            scored.sort_by(|a, b| b.1.cmp(&a.1));
            self.filtered_artist_indices = scored.into_iter().map(|(i, _)| i).collect();
        }
        drop(st);

        if self.filtered_artist_indices.is_empty() {
            self.artist_list_state.select(None);
        } else if let Some(sel) = self.artist_list_state.selected() {
            if sel >= self.filtered_artist_indices.len() {
                self.artist_list_state
                    .select(Some(self.filtered_artist_indices.len() - 1));
            }
        } else {
            self.artist_list_state.select(Some(0));
        }
    }

    fn update_artist_track_filter(&mut self) {
        let st = self.state.lock().unwrap();
        let query = &self.search_query;
        if query.is_empty() {
            self.filtered_artist_track_indices = (0..st.artist_tracks.len()).collect();
        } else {
            let mut scored: Vec<(usize, i32)> = st
                .artist_tracks
                .iter()
                .enumerate()
                .filter_map(|(i, t)| fuzzy_score(query, &t.name).map(|s| (i, s)))
                .collect();
            scored.sort_by(|a, b| b.1.cmp(&a.1));
            self.filtered_artist_track_indices = scored.into_iter().map(|(i, _)| i).collect();
        }
        drop(st);

        if self.filtered_artist_track_indices.is_empty() {
            self.artist_track_list_state.select(None);
        } else if let Some(sel) = self.artist_track_list_state.selected() {
            if sel >= self.filtered_artist_track_indices.len() {
                self.artist_track_list_state
                    .select(Some(self.filtered_artist_track_indices.len() - 1));
            }
        } else {
            self.artist_track_list_state.select(Some(0));
        }
    }

    fn cycle_theme(&mut self) {
        self.theme_name = self.theme_name.next();
        self.theme = self.theme_name.theme();
        self.set_status(&format!("Theme: {}", self.theme_name.label()));
        save_config_theme(self.theme_name);
    }

    fn update_filter(&mut self) {
        let st = self.state.lock().unwrap();
        let query = &self.search_query;
        if query.is_empty() {
            self.filtered_indices = (0..st.playlists.len()).collect();
        } else {
            let mut scored: Vec<(usize, i32)> = st
                .playlists
                .iter()
                .enumerate()
                .filter_map(|(i, name)| fuzzy_score(query, name).map(|s| (i, s)))
                .collect();
            scored.sort_by(|a, b| b.1.cmp(&a.1));
            self.filtered_indices = scored.into_iter().map(|(i, _)| i).collect();
        }
        drop(st);

        if self.filtered_indices.is_empty() {
            self.list_state.select(None);
        } else if let Some(sel) = self.list_state.selected() {
            if sel >= self.filtered_indices.len() {
                self.list_state.select(Some(self.filtered_indices.len() - 1));
            }
        } else {
            self.list_state.select(Some(0));
        }
    }

    fn update_track_filter(&mut self) {
        let st = self.state.lock().unwrap();
        let query = &self.search_query;
        if query.is_empty() {
            self.filtered_track_indices = (0..st.playlist_tracks.len()).collect();
        } else {
            let mut scored: Vec<(usize, i32)> = st
                .playlist_tracks
                .iter()
                .enumerate()
                .filter_map(|(i, t)| {
                    let name_score = fuzzy_score(query, &t.name);
                    let artist_score = fuzzy_score(query, &t.artist);
                    let best = match (name_score, artist_score) {
                        (Some(a), Some(b)) => Some(a.max(b)),
                        (Some(a), None) => Some(a),
                        (None, Some(b)) => Some(b),
                        (None, None) => None,
                    };
                    best.map(|s| (i, s))
                })
                .collect();
            scored.sort_by(|a, b| b.1.cmp(&a.1));
            self.filtered_track_indices = scored.into_iter().map(|(i, _)| i).collect();
        }
        drop(st);

        if self.filtered_track_indices.is_empty() {
            self.track_list_state.select(None);
        } else if let Some(sel) = self.track_list_state.selected() {
            if sel >= self.filtered_track_indices.len() {
                self.track_list_state
                    .select(Some(self.filtered_track_indices.len() - 1));
            }
        } else {
            self.track_list_state.select(Some(0));
        }
    }

    fn selected_playlist_name(&self) -> Option<String> {
        let sel = self.list_state.selected()?;
        let idx = *self.filtered_indices.get(sel)?;
        let st = self.state.lock().unwrap();
        st.playlists.get(idx).cloned()
    }

    fn selected_track(&self) -> Option<PlaylistTrack> {
        let sel = self.track_list_state.selected()?;
        let idx = *self.filtered_track_indices.get(sel)?;
        let st = self.state.lock().unwrap();
        st.playlist_tracks.get(idx).cloned()
    }

    fn set_status(&self, msg: &str) {
        let mut st = self.state.lock().unwrap();
        st.status = msg.to_string();
        st.status_time = Instant::now();
    }
}

// ── Fuzzy matching ─────────────────────────────────────────────────

fn fuzzy_score(query: &str, target: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let query_lower: Vec<char> = query.to_lowercase().chars().collect();
    let target_lower: Vec<char> = target.to_lowercase().chars().collect();
    let target_chars: Vec<char> = target.chars().collect();

    let mut qi = 0;
    let mut score: i32 = 0;
    let mut prev_match_idx: Option<usize> = None;

    for (ti, &tc) in target_lower.iter().enumerate() {
        if qi < query_lower.len() && tc == query_lower[qi] {
            score += 1;
            // Bonus for consecutive matches
            if let Some(prev) = prev_match_idx {
                if ti == prev + 1 {
                    score += 5;
                }
            }
            // Bonus for matching at word boundary
            if ti == 0 || !target_chars[ti - 1].is_alphanumeric() {
                score += 3;
            }
            // Bonus for case-exact match
            if target_chars[ti] == query.chars().nth(qi).unwrap_or(' ') {
                score += 1;
            }
            prev_match_idx = Some(ti);
            qi += 1;
        }
    }

    if qi == query_lower.len() {
        // Bonus for shorter targets (tighter matches)
        score += (100i32).saturating_sub(target.len() as i32);
        Some(score)
    } else {
        None
    }
}

// ── AppleScript helpers ────────────────────────────────────────────

fn run_applescript(script: &str) -> Result<String, String> {
    let output = Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(script)
        .output();

    match output {
        Ok(o) => {
            if o.status.success() {
                Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                Err(String::from_utf8_lossy(&o.stderr).trim().to_string())
            }
        }
        Err(e) => Err(e.to_string()),
    }
}

fn applescript_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', " ")
        .replace('\r', " ")
}

fn parse_number(s: &str) -> f64 {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return 0.0;
    }
    let text = if trimmed.contains(',') && !trimmed.contains('.') {
        trimmed.replace(',', ".")
    } else {
        trimmed.replace(',', "")
    };
    let filtered: String = text
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    filtered.parse().unwrap_or(0.0)
}

// ── AppleScript fetchers ───────────────────────────────────────────

fn fetch_now_playing() -> TrackInfo {
    let script = format!(
        r#"tell application "{}"
    if it is running then
        if player state is stopped then return "STOPPED"
        set t to current track
        set lv to false
        try
            set lv to loved of t
        end try
        return name of t & "\n" & artist of t & "\n" & album of t & "\n" & (player state as string) & "\n" & duration of t & "\n" & player position & "\n" & (lv as string)
    end if
end tell
return "NOT_RUNNING""#,
        APP_NAME
    );

    match run_applescript(&script) {
        Ok(out) => {
            if out == "STOPPED" || out == "NOT_RUNNING" || out.is_empty() {
                return TrackInfo {
                    state: PlayerState::from_str(&out),
                    ..Default::default()
                };
            }
            let parts: Vec<&str> = out.split('\n').collect();
            if parts.len() >= 6 {
                let loved = parts.get(6).and_then(|s| match s.trim() {
                    "true" => Some(true),
                    "false" => Some(false),
                    _ => None,
                });
                TrackInfo {
                    name: parts[0].to_string(),
                    artist: parts[1].to_string(),
                    album: parts[2].to_string(),
                    state: PlayerState::from_str(parts[3]),
                    duration: parse_number(parts[4]),
                    position: parse_number(parts[5]),
                    loved,
                }
            } else {
                TrackInfo::default()
            }
        }
        Err(_) => TrackInfo::default(),
    }
}

fn fetch_playlists() -> Vec<String> {
    let script = format!(
        r#"set AppleScript's text item delimiters to "\n"
tell application "{}"
    if it is running then return name of playlists as text
end tell
return "NOT_RUNNING""#,
        APP_NAME
    );

    match run_applescript(&script) {
        Ok(out) => {
            if out == "NOT_RUNNING" || out.is_empty() {
                Vec::new()
            } else {
                out.split('\n')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            }
        }
        Err(_) => Vec::new(),
    }
}

fn fetch_up_next() -> (String, String) {
    // UI approach
    let script = format!(
        r#"tell application "System Events"
    if not (exists process "{}") then return "NO"
    tell process "{}"
        if not (exists window 1) then return "NO"
        try
            set theTable to first table of scroll area 1 of window 1
            set row1 to first row of theTable
            set texts to value of static text of row1
            if (count of texts) >= 2 then
                return item 1 of texts & "\n" & item 2 of texts
            else if (count of texts) = 1 then
                return item 1 of texts
            end if
        end try
    end tell
end tell
return "NO""#,
        APP_NAME, APP_NAME
    );

    if let Ok(out) = run_applescript(&script) {
        if out != "NO" && !out.is_empty() {
            let parts: Vec<&str> = out.split('\n').collect();
            return (
                parts.first().unwrap_or(&"").to_string(),
                parts.get(1).unwrap_or(&"").to_string(),
            );
        }
    }

    // Playlist fallback
    let script = format!(
        r#"tell application "{}"
    if it is running then
        if player state is stopped then return "NO"
        try
            set cp to current playlist
            set ct to current track
            set pid to persistent ID of ct
            set tl to tracks of cp
            repeat with i from 1 to count of tl
                if persistent ID of item i of tl is pid then
                    if i < count of tl then
                        set nt to item (i + 1) of tl
                        return name of nt & "\n" & artist of nt
                    end if
                end if
            end repeat
        end try
    end if
end tell
return "NO""#,
        APP_NAME
    );

    match run_applescript(&script) {
        Ok(out) => {
            if out == "NO" || out.is_empty() {
                (String::new(), String::new())
            } else {
                let parts: Vec<&str> = out.split('\n').collect();
                (
                    parts.first().unwrap_or(&"").to_string(),
                    parts.get(1).unwrap_or(&"").to_string(),
                )
            }
        }
        Err(_) => (String::new(), String::new()),
    }
}

fn fetch_shuffle() -> Option<bool> {
    let script = format!(
        r#"tell application "{}"
    if it is running then
        try
            set p to current playlist
            return shuffle enabled of p as string
        on error
            try
                return shuffle enabled as string
            end try
        end try
    end if
end tell
return "UNKNOWN""#,
        APP_NAME
    );

    match run_applescript(&script) {
        Ok(out) => match out.as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        Err(_) => None,
    }
}

fn fetch_repeat() -> Option<String> {
    let script = format!(
        r#"tell application "{}"
    if it is running then
        try
            return song repeat as string
        end try
    end if
end tell
return "UNKNOWN""#,
        APP_NAME
    );

    match run_applescript(&script) {
        Ok(out) => match out.trim().to_lowercase().as_str() {
            "off" | "none" => Some("off".into()),
            "one" => Some("one".into()),
            "all" => Some("all".into()),
            _ => None,
        },
        Err(_) => None,
    }
}

fn fetch_volume() -> i32 {
    let script = r#"output volume of (get volume settings)"#;
    match run_applescript(script) {
        Ok(out) => parse_number(&out) as i32,
        Err(_) => -1,
    }
}

fn fetch_current_playlist() -> String {
    let script = format!(
        r#"tell application "{}"
    if it is running then
        if player state is not stopped then
            try
                return name of current playlist
            end try
        end if
    end if
end tell
return """#,
        APP_NAME
    );

    run_applescript(&script).unwrap_or_default()
}

fn fetch_queue(max_items: usize) -> Vec<(String, String)> {
    // Read the actual play queue from Music UI (respects shuffle order)
    let script = format!(
        r#"tell application "System Events"
    tell process "{app}"
        set sg to splitter group 1 of window 1

        -- Open playing next panel if closed
        set wasOpen to true
        set allGroups to groups of sg
        repeat with g in allGroups
            try
                set cbs to checkboxes of g
                repeat with cb in cbs
                    if description of cb is "playing next" then
                        if value of cb is 0 then
                            set wasOpen to false
                            click cb
                            delay 1
                        end if
                    end if
                end repeat
            end try
        end repeat

        set out to ""
        try
            set g3 to group 3 of sg
            set sa to scroll area 1 of g3
            set tb to table 1 of sa
            set allRows to rows of tb
            set cnt to 0
            repeat with r in allRows
                try
                    set txts to value of static text of UI element 1 of r
                    if (count of txts) >= 2 then
                        set songName to item 1 of txts as string
                        -- Skip header rows
                        if songName is not "History" and songName is not "Playing Next" and songName is not "Autoplay" then
                            set artistAlbum to item 2 of txts as string
                            -- Extract artist (before " — ")
                            set oldDelims to AppleScript's text item delimiters
                            set AppleScript's text item delimiters to " — "
                            try
                                set artistPart to text item 1 of artistAlbum
                            on error
                                set artistPart to artistAlbum
                            end try
                            set AppleScript's text item delimiters to oldDelims
                            set out to out & songName & "\t" & artistPart & "\n"
                            set cnt to cnt + 1
                            if cnt >= {max} then exit repeat
                        end if
                    end if
                end try
            end repeat
        end try

        -- Close panel if we opened it
        if not wasOpen then
            set allGroups to groups of sg
            repeat with g in allGroups
                try
                    set cbs to checkboxes of g
                    repeat with cb in cbs
                        if description of cb is "playing next" then
                            click cb
                            exit repeat
                        end if
                    end repeat
                end try
            end repeat
        end if

        return out
    end tell
end tell"#,
        app = APP_NAME,
        max = max_items
    );

    match run_applescript(&script) {
        Ok(out) => {
            if out.is_empty() {
                return Vec::new();
            }
            out.lines()
                .filter_map(|line| {
                    let parts: Vec<&str> = line.split('\t').collect();
                    if parts.len() >= 2 {
                        Some((parts[0].to_string(), parts[1].to_string()))
                    } else {
                        None
                    }
                })
                .collect()
        }
        Err(_) => Vec::new(),
    }
}

fn fetch_playlist_tracks(playlist_name: &str) -> Vec<PlaylistTrack> {
    let safe = applescript_escape(playlist_name);
    let script = format!(
        r#"tell application "{}"
    if it is running then
        try
            set p to playlist "{}"
            set tl to tracks of p
            set out to ""
            repeat with i from 1 to count of tl
                set t to item i of tl
                set out to out & name of t & "\t" & artist of t & "\t" & (duration of t as string) & "\n"
            end repeat
            return out
        end try
    end if
end tell
return "NONE""#,
        APP_NAME, safe
    );

    match run_applescript(&script) {
        Ok(out) => {
            if out == "NONE" || out.is_empty() {
                return Vec::new();
            }
            out.lines()
                .enumerate()
                .filter_map(|(i, line)| {
                    let parts: Vec<&str> = line.split('\t').collect();
                    if parts.len() >= 3 {
                        Some(PlaylistTrack {
                            name: parts[0].to_string(),
                            artist: parts[1].to_string(),
                            duration: parse_number(parts[2]),
                            index: i + 1,
                        })
                    } else {
                        None
                    }
                })
                .collect()
        }
        Err(_) => Vec::new(),
    }
}

// ── AppleScript commands ───────────────────────────────────────────

fn cmd_play_pause() {
    let _ = run_applescript(&format!(
        r#"tell application "{}" to playpause"#,
        APP_NAME
    ));
}

fn cmd_next() {
    let _ = run_applescript(&format!(
        r#"tell application "{}" to next track"#,
        APP_NAME
    ));
}

fn cmd_prev() {
    let _ = run_applescript(&format!(
        r#"tell application "{}" to previous track"#,
        APP_NAME
    ));
}

fn cmd_stop() {
    let _ = run_applescript(&format!(
        r#"tell application "{}" to stop"#,
        APP_NAME
    ));
}

fn cmd_play_playlist(name: &str) {
    let safe = applescript_escape(name);
    let _ = run_applescript(&format!(
        r#"tell application "{}" to play playlist "{}""#,
        APP_NAME, safe
    ));
}

fn cmd_play_track_in_playlist(playlist: &str, track_index: usize) {
    let safe = applescript_escape(playlist);
    let _ = run_applescript(&format!(
        r#"tell application "{}"
    play track {} of playlist "{}"
end tell"#,
        APP_NAME, track_index, safe
    ));
}

fn cmd_set_volume(vol: i32) {
    let _ = run_applescript(&format!("set volume output volume {}", vol));
}

fn cmd_seek(pos: f64) {
    let _ = run_applescript(&format!(
        r#"tell application "{}" to set player position to {}"#,
        APP_NAME, pos
    ));
}

fn cmd_toggle_shuffle() {
    let script = format!(
        r#"tell application "{}"
    if it is running then
        try
            set p to current playlist
            set shuffle enabled of p to not shuffle enabled of p
        on error
            try
                set shuffle enabled to not shuffle enabled
            end try
        end try
    end if
end tell"#,
        APP_NAME
    );
    let _ = run_applescript(&script);
}

fn cmd_toggle_love() {
    let script = format!(
        r#"tell application "{}"
    if it is running then
        if player state is not stopped then
            set t to current track
            try
                set loved of t to not loved of t
            end try
        end if
    end if
end tell"#,
        APP_NAME
    );
    let _ = run_applescript(&script);
}

fn fetch_airplay_devices() -> Vec<(String, bool)> {
    let script = format!(
        r#"tell application "{}"
    if it is running then
        try
            set devs to AirPlay devices
            set out to ""
            repeat with d in devs
                set out to out & name of d & "\t" & (selected of d as string) & "\n"
            end repeat
            return out
        end try
    end if
end tell
return "NONE""#,
        APP_NAME
    );

    match run_applescript(&script) {
        Ok(out) => {
            if out == "NONE" || out.is_empty() {
                return Vec::new();
            }
            out.lines()
                .filter_map(|line| {
                    let parts: Vec<&str> = line.split('\t').collect();
                    if parts.len() >= 2 {
                        Some((parts[0].to_string(), parts[1] == "true"))
                    } else {
                        None
                    }
                })
                .collect()
        }
        Err(_) => Vec::new(),
    }
}

fn fetch_artists() -> Vec<String> {
    let script = format!(
        r#"tell application "{}"
    if it is running then
        try
            set allArtists to artist of tracks of playlist "Library"
            set uniqueArts to {{}}
            repeat with a in allArtists
                set artStr to a as string
                if artStr is not in uniqueArts and artStr is not "" then
                    set end of uniqueArts to artStr
                end if
            end repeat
            set AppleScript's text item delimiters to "\n"
            return uniqueArts as text
        end try
    end if
end tell
return "NONE""#,
        APP_NAME
    );

    match run_applescript(&script) {
        Ok(out) => {
            if out == "NONE" || out.is_empty() {
                return Vec::new();
            }
            let mut artists: Vec<String> = out
                .lines()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            artists.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
            artists
        }
        Err(_) => Vec::new(),
    }
}

fn fetch_artist_tracks(artist: &str) -> Vec<PlaylistTrack> {
    let safe = applescript_escape(artist);
    let script = format!(
        r#"tell application "{}"
    if it is running then
        try
            set results to (every track of playlist "Library" whose artist is "{}")
            set out to ""
            set idx to 0
            repeat with t in results
                set idx to idx + 1
                set out to out & name of t & "\t" & artist of t & "\t" & (duration of t as string) & "\t" & (idx as string) & "\n"
            end repeat
            return out
        end try
    end if
end tell
return "NONE""#,
        APP_NAME, safe
    );

    match run_applescript(&script) {
        Ok(out) => {
            if out == "NONE" || out.is_empty() {
                return Vec::new();
            }
            out.lines()
                .filter_map(|line| {
                    let parts: Vec<&str> = line.split('\t').collect();
                    if parts.len() >= 3 {
                        Some(PlaylistTrack {
                            name: parts[0].to_string(),
                            artist: parts[1].to_string(),
                            duration: parse_number(parts[2]),
                            index: parts.get(3).map(|s| parse_number(s) as usize).unwrap_or(0),
                        })
                    } else {
                        None
                    }
                })
                .collect()
        }
        Err(_) => Vec::new(),
    }
}

fn fetch_library_search(query: &str, max_results: usize) -> Vec<PlaylistTrack> {
    let safe = applescript_escape(query);
    let script = format!(
        r#"tell application "{}"
    if it is running then
        try
            set results to (search playlist "Library" for "{}" only songs)
            set out to ""
            set cnt to 0
            repeat with t in results
                set out to out & name of t & "\t" & artist of t & "\t" & (duration of t as string) & "\t" & (index of t as string) & "\n"
                set cnt to cnt + 1
                if cnt >= {} then exit repeat
            end repeat
            return out
        end try
    end if
end tell
return "NONE""#,
        APP_NAME, safe, max_results
    );

    match run_applescript(&script) {
        Ok(out) => {
            if out == "NONE" || out.is_empty() {
                return Vec::new();
            }
            out.lines()
                .filter_map(|line| {
                    let parts: Vec<&str> = line.split('\t').collect();
                    if parts.len() >= 3 {
                        Some(PlaylistTrack {
                            name: parts[0].to_string(),
                            artist: parts[1].to_string(),
                            duration: parse_number(parts[2]),
                            index: parts.get(3).map(|s| parse_number(s) as usize).unwrap_or(0),
                        })
                    } else {
                        None
                    }
                })
                .collect()
        }
        Err(_) => Vec::new(),
    }
}

fn cmd_play_library_track(track_name: &str, artist: &str) {
    let safe_name = applescript_escape(track_name);
    let safe_artist = applescript_escape(artist);
    let script = format!(
        r#"tell application "{}"
    if it is running then
        try
            set results to (search playlist "Library" for "{}" only songs)
            repeat with t in results
                if name of t is "{}" and artist of t is "{}" then
                    play t
                    return "OK"
                end if
            end repeat
        end try
    end if
end tell
return "FAIL""#,
        APP_NAME, safe_name, safe_name, safe_artist
    );
    let _ = run_applescript(&script);
}

fn cmd_toggle_airplay(device_name: &str) {
    let safe = applescript_escape(device_name);
    let script = format!(
        r#"tell application "{}"
    if it is running then
        try
            set d to (first AirPlay device whose name is "{}")
            set selected of d to not selected of d
        end try
    end if
end tell"#,
        APP_NAME, safe
    );
    let _ = run_applescript(&script);
}

fn cmd_set_repeat(mode: &str) {
    let _ = run_applescript(&format!(
        r#"tell application "{}" to set song repeat to {}"#,
        APP_NAME, mode
    ));
}

fn cmd_add_to_playlist(playlist_name: &str) {
    let safe = applescript_escape(playlist_name);
    let _ = run_applescript(&format!(
        r#"tell application "{app}"
    set ct to current track
    duplicate ct to playlist "{pl}"
end tell"#,
        app = APP_NAME,
        pl = safe,
    ));
}

// ── Format helpers ─────────────────────────────────────────────────

fn format_time(seconds: f64) -> String {
    if seconds <= 0.0 {
        return "0:00".into();
    }
    let total = seconds as u64;
    let m = total / 60;
    let s = total % 60;
    format!("{}:{:02}", m, s)
}

// ── Drawing ────────────────────────────────────────────────────────

fn draw(f: &mut Frame, app: &mut App) {
    let size = f.area();
    f.render_widget(Block::default().style(Style::default().bg(app.theme.surface)), size);

    if app.mini_mode {
        draw_mini(f, size, app);
        return;
    }

    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(10),
            Constraint::Length(1),
        ])
        .split(size);

    draw_header(f, main_chunks[0], app);
    draw_body(f, main_chunks[1], app);
    draw_status_bar(f, main_chunks[2], app);

    if app.show_help {
        draw_help_overlay(f, size, &app.theme);
    }
    if app.show_airplay {
        draw_airplay_overlay(f, size, app);
    }
    if app.show_add_to_playlist {
        draw_add_to_playlist_overlay(f, size, app);
    }
}

fn draw_mini(f: &mut Frame, area: Rect, app: &App) {
    let th = &app.theme;
    let st = app.state.lock().unwrap();
    let t = &st.track;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1), Constraint::Min(0)])
        .split(area);

    // Line 1: state icon + track name + artist + duration
    let icon = t.state.icon();
    let pos_str = format_time(t.position);
    let dur_str = format_time(t.duration);

    let mut spans = vec![
        Span::styled(format!(" {} ", icon), Style::default().fg(t.state.color(th)).bold()),
    ];

    if t.name.is_empty() {
        spans.push(Span::styled("No track", Style::default().fg(th.dim)));
    } else {
        spans.push(Span::styled(&t.name, Style::default().fg(th.text).bold()));
        spans.push(Span::styled(" · ", Style::default().fg(th.dim)));
        spans.push(Span::styled(&t.artist, Style::default().fg(th.text_dim)));
    }

    if t.duration > 0.0 {
        let time_str = format!("  {}/{}", pos_str, dur_str);
        spans.push(Span::styled(time_str, Style::default().fg(th.text_dim)));
    }

    if t.loved == Some(true) {
        spans.push(Span::styled(" ♥", Style::default().fg(th.red)));
    }

    f.render_widget(Paragraph::new(Line::from(spans)), chunks[0]);

    // Line 2: progress bar
    if t.duration > 0.0 {
        let ratio = (t.position / t.duration).clamp(0.0, 1.0);
        let bar_width = area.width.saturating_sub(2) as usize;
        let filled = (ratio * bar_width as f64) as usize;
        let empty = bar_width.saturating_sub(filled);
        let bar = Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled("━".repeat(filled), Style::default().fg(th.accent)),
            Span::styled("╌".repeat(empty), Style::default().fg(th.dim)),
        ]);
        f.render_widget(Paragraph::new(bar), chunks[1]);
    }
}

fn draw_header(f: &mut Frame, area: Rect, app: &App) {
    let th = &app.theme;
    let st = app.state.lock().unwrap();
    let state_str: String = match st.track.state {
        PlayerState::Playing => " ● playing ".into(),
        PlayerState::Paused => " ⏸ paused ".into(),
        PlayerState::Stopped => " ⏹ stopped ".into(),
        PlayerState::NotRunning => " ○ not running ".into(),
    };
    let state_color = st.track.state.color(th);
    drop(st);

    let title = " ♫ Apple Music ";
    let pad_len = area
        .width
        .saturating_sub(title.len() as u16 + state_str.len() as u16) as usize;

    let header = Line::from(vec![
        Span::styled(title, Style::default().fg(th.accent).bold()),
        Span::styled(" ".repeat(pad_len), Style::default().bg(th.surface_light)),
        Span::styled(state_str, Style::default().fg(state_color).bold()),
    ]);

    f.render_widget(
        Paragraph::new(header).style(Style::default().bg(th.surface_light)),
        area,
    );
}

fn draw_body(f: &mut Frame, area: Rect, app: &mut App) {
    let show_sidebar = area.width >= 72;

    if show_sidebar {
        let body_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(40), Constraint::Length(32)])
            .split(area);
        draw_left_panel(f, body_chunks[0], app);
        draw_right_panel(f, body_chunks[1], app);
    } else {
        draw_left_panel(f, area, app);
    }
}

fn draw_left_panel(f: &mut Frame, area: Rect, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Min(5),
        ])
        .horizontal_margin(1)
        .split(area);

    draw_now_playing(f, chunks[0], app);
    draw_progress_bar(f, chunks[1], app);
    draw_controls(f, chunks[2], app);
    draw_playlist(f, chunks[3], app);
}

fn draw_now_playing(f: &mut Frame, area: Rect, app: &App) {
    let th = &app.theme;
    let st = app.state.lock().unwrap();
    let t = &st.track;

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(th.border))
        .style(Style::default().bg(th.surface));

    let inner = block.inner(area);
    f.render_widget(block, area);

    if t.state == PlayerState::NotRunning || t.state == PlayerState::Stopped {
        let msg = if t.state == PlayerState::NotRunning {
            "Music app is not running"
        } else {
            "Nothing playing"
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(msg, Style::default().fg(th.dim)))),
            inner,
        );
        return;
    }

    let mut title_spans = vec![Span::styled(
        t.name.clone(),
        Style::default().fg(th.text).bold(),
    )];
    if t.loved == Some(true) {
        title_spans.push(Span::styled("  ♥", Style::default().fg(th.red)));
    }
    let title = Line::from(title_spans);
    let subtitle = Line::from(vec![
        Span::styled(t.artist.clone(), Style::default().fg(th.text_dim)),
        Span::styled("  ·  ", Style::default().fg(th.dim)),
        Span::styled(t.album.clone(), Style::default().fg(th.text_dim)),
    ]);
    let state_line = Line::from(Span::styled(
        format!("{} {}", t.state.icon(), t.state.label()),
        Style::default().fg(t.state.color(th)),
    ));

    f.render_widget(
        Paragraph::new(vec![title, subtitle, Line::from(""), state_line]),
        inner,
    );
}

fn draw_progress_bar(f: &mut Frame, area: Rect, app: &mut App) {
    app.progress_bar_area = Some(area);
    let th = &app.theme;
    let st = app.state.lock().unwrap();
    let t = &st.track;

    if t.duration <= 0.0 {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  ╌╌╌ no track ╌╌╌",
                Style::default().fg(th.dim),
            ))),
            area,
        );
        return;
    }

    let ratio = (t.position / t.duration).clamp(0.0, 1.0);
    let pos_str = format_time(t.position);
    let dur_str = format_time(t.duration);

    let bar_width =
        area.width.saturating_sub(pos_str.len() as u16 + dur_str.len() as u16 + 6) as usize;
    let filled = (ratio * bar_width as f64) as usize;
    let empty = bar_width.saturating_sub(filled + 1);

    let bar = Line::from(vec![
        Span::styled(format!("  {} ", pos_str), Style::default().fg(th.text_dim)),
        Span::styled("━".repeat(filled), Style::default().fg(th.accent)),
        Span::styled("●", Style::default().fg(th.text).bold()),
        Span::styled("╌".repeat(empty), Style::default().fg(th.dim)),
        Span::styled(format!(" {} ", dur_str), Style::default().fg(th.text_dim)),
    ]);

    f.render_widget(Paragraph::new(bar), area);
}

fn draw_controls(f: &mut Frame, area: Rect, app: &App) {
    let th = &app.theme;
    let st = app.state.lock().unwrap();

    let mut spans = Vec::new();
    spans.push(Span::styled("  ", Style::default()));

    match st.shuffle {
        Some(true) => spans.push(Span::styled("⇆ On ", Style::default().fg(th.green).bold())),
        Some(false) => spans.push(Span::styled("⇆ Off ", Style::default().fg(th.dim))),
        None => spans.push(Span::styled("⇆ ─ ", Style::default().fg(th.dim))),
    }

    spans.push(Span::styled("   ", Style::default()));

    match st.repeat_mode.as_deref() {
        Some("all") => spans.push(Span::styled("↻ All ", Style::default().fg(th.green).bold())),
        Some("one") => spans.push(Span::styled("↻ One ", Style::default().fg(th.yellow).bold())),
        _ => spans.push(Span::styled("↻ Off ", Style::default().fg(th.dim))),
    }

    spans.push(Span::styled("   ", Style::default()));

    if st.volume >= 0 {
        let vol_icon = if st.volume == 0 {
            "🔇"
        } else if st.volume < 30 {
            "🔈"
        } else if st.volume < 70 {
            "🔉"
        } else {
            "🔊"
        };
        let bar_width = 10;
        let filled = (st.volume as usize * bar_width) / 100;
        let empty = bar_width - filled;
        let vol_color = if st.volume == 0 {
            th.red
        } else if st.volume < 30 {
            th.text_dim
        } else if st.volume < 70 {
            th.accent
        } else {
            th.green
        };
        spans.push(Span::styled(
            format!("{} ", vol_icon),
            Style::default().fg(vol_color),
        ));
        spans.push(Span::styled(
            "█".repeat(filled),
            Style::default().fg(vol_color),
        ));
        spans.push(Span::styled(
            "░".repeat(empty),
            Style::default().fg(th.dim),
        ));
        spans.push(Span::styled(
            format!(" {}%", st.volume),
            Style::default().fg(th.text_dim),
        ));
    }

    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_playlist(f: &mut Frame, area: Rect, app: &mut App) {
    match &app.browse_view {
        BrowseView::Playlists => draw_playlist_list(f, area, app),
        BrowseView::Tracks(_) => draw_track_list(f, area, app),
        BrowseView::GlobalSearch => draw_global_search(f, area, app),
        BrowseView::Artists => draw_artist_list(f, area, app),
        BrowseView::ArtistTracks(_) => draw_artist_tracks(f, area, app),
        BrowseView::RecentlyPlayed => draw_recently_played(f, area, app),
    }
}

fn draw_recently_played(f: &mut Frame, area: Rect, app: &mut App) {
    let th = app.theme;
    let st = app.state.lock().unwrap();
    let recent = st.recent_tracks.clone();
    drop(st);

    let title = format!(" Recently Played ({}) ", recent.len());

    let block = Block::default()
        .title(Span::styled(title, Style::default().fg(th.accent).bold()))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(th.border))
        .style(Style::default().bg(th.surface));

    if recent.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled(
                "  No recently played tracks yet",
                Style::default().fg(th.dim),
            ))
            .block(block),
            area,
        );
        return;
    }

    let items: Vec<ListItem> = recent
        .iter()
        .enumerate()
        .map(|(i, (name, artist))| {
            let num = format!(" {:>2}. ", i + 1);
            let max_name = (area.width as usize).saturating_sub(num.len() + artist.len() + 6);
            let display_name: String = name.chars().take(max_name).collect();
            ListItem::new(Line::from(vec![
                Span::styled(num, Style::default().fg(th.dim)),
                Span::styled(display_name, Style::default().fg(th.text)),
                Span::styled(" — ", Style::default().fg(th.dim)),
                Span::styled(artist.clone(), Style::default().fg(th.dim)),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(th.highlight_bg)
                .fg(th.accent)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");

    f.render_stateful_widget(list, area, &mut app.recent_list_state);
}

fn draw_artist_list(f: &mut Frame, area: Rect, app: &mut App) {
    let th = app.theme;
    let st = app.state.lock().unwrap();
    let artists = st.artists.clone();
    drop(st);

    let title = match &app.input_mode {
        InputMode::Search => {
            let blink = if (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                / 500)
                % 2
                == 0
            { "▏" } else { " " };
            format!(" Search: {}{} ", app.search_query, blink)
        }
        InputMode::Normal => format!(" Artists ({}) ◂ Esc ", app.filtered_artist_indices.len()),
    };

    let border_color = match &app.input_mode {
        InputMode::Search => th.accent,
        InputMode::Normal => th.accent,
    };

    let block = Block::default()
        .title(Span::styled(&title, Style::default().fg(th.text_dim).bold()))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .style(Style::default().bg(th.surface));

    if artists.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled("  Loading artists...", Style::default().fg(th.dim)))
                .block(block),
            area,
        );
        return;
    }

    let items: Vec<ListItem> = app
        .filtered_artist_indices
        .iter()
        .filter_map(|&idx| {
            let name = artists.get(idx)?;
            Some(ListItem::new(Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(name.clone(), Style::default().fg(th.text)),
            ])))
        })
        .collect();

    let total = items.len();
    let inner_height = area.height.saturating_sub(2) as usize;

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(th.highlight_bg)
                .fg(th.accent)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");

    f.render_stateful_widget(list, area, &mut app.artist_list_state);

    if total > inner_height {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .thumb_style(Style::default().fg(th.accent))
            .track_style(Style::default().fg(th.border));
        let mut scrollbar_state = ScrollbarState::new(total)
            .position(app.artist_list_state.selected().unwrap_or(0));
        let scroll_area = Rect {
            x: area.x, y: area.y + 1,
            width: area.width, height: area.height.saturating_sub(2),
        };
        f.render_stateful_widget(scrollbar, scroll_area, &mut scrollbar_state);
    }
}

fn draw_artist_tracks(f: &mut Frame, area: Rect, app: &mut App) {
    let th = app.theme;
    let st = app.state.lock().unwrap();
    let tracks = st.artist_tracks.clone();
    let now_playing = st.track.name.clone();
    let now_artist = st.track.artist.clone();
    drop(st);

    let artist_name = if let BrowseView::ArtistTracks(ref name) = app.browse_view {
        name.clone()
    } else {
        String::new()
    };

    let title = format!(" {} ({}) ◂ Esc ", artist_name, app.filtered_artist_track_indices.len());

    let block = Block::default()
        .title(Span::styled(&title, Style::default().fg(th.text_dim).bold()))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(th.accent))
        .style(Style::default().bg(th.surface));

    if tracks.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled("  Loading...", Style::default().fg(th.dim)))
                .block(block),
            area,
        );
        return;
    }

    let max_width = area.width.saturating_sub(8) as usize;

    let items: Vec<ListItem> = app
        .filtered_artist_track_indices
        .iter()
        .filter_map(|&idx| {
            let t = tracks.get(idx)?;
            let is_playing = !now_playing.is_empty() && t.name == now_playing && t.artist == now_artist;
            let dur = format_time(t.duration);
            let prefix = if is_playing { "▶ " } else { "  " };
            let name_max = max_width.saturating_sub(dur.len() + prefix.len() + 3);
            let name_display = if t.name.chars().count() > name_max {
                let limit = name_max.saturating_sub(1);
                let truncated: String = t.name.chars().take(limit).collect();
                format!("{}…", truncated)
            } else {
                t.name.clone()
            };

            let style = if is_playing { Style::default().fg(th.green) } else { Style::default().fg(th.text) };
            let dim_style = if is_playing { Style::default().fg(th.green) } else { Style::default().fg(th.text_dim) };

            Some(ListItem::new(Line::from(vec![
                Span::styled(prefix, style),
                Span::styled(name_display, style),
                Span::styled(format!("  {}", dur), dim_style),
            ])))
        })
        .collect();

    let total = items.len();
    let inner_height = area.height.saturating_sub(2) as usize;

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(th.highlight_bg)
                .fg(th.accent)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");

    f.render_stateful_widget(list, area, &mut app.artist_track_list_state);

    if total > inner_height {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .thumb_style(Style::default().fg(th.accent))
            .track_style(Style::default().fg(th.border));
        let mut scrollbar_state = ScrollbarState::new(total)
            .position(app.artist_track_list_state.selected().unwrap_or(0));
        let scroll_area = Rect {
            x: area.x, y: area.y + 1,
            width: area.width, height: area.height.saturating_sub(2),
        };
        f.render_stateful_widget(scrollbar, scroll_area, &mut scrollbar_state);
    }
}

fn draw_playlist_list(f: &mut Frame, area: Rect, app: &mut App) {
    let th = app.theme;
    let st = app.state.lock().unwrap();
    let playlists = st.playlists.clone();
    let current = st.current_playlist.clone();
    drop(st);

    let title = match &app.input_mode {
        InputMode::Search => {
            let blink = if (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                / 500)
                % 2
                == 0
            {
                "▏"
            } else {
                " "
            };
            format!(" Search: {}{} ", app.search_query, blink)
        }
        InputMode::Normal => format!(" Library ({}) ", app.filtered_indices.len()),
    };

    let border_color = match &app.input_mode {
        InputMode::Search => th.accent,
        InputMode::Normal => th.border,
    };

    let block = Block::default()
        .title(Span::styled(&title, Style::default().fg(th.text_dim).bold()))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .style(Style::default().bg(th.surface));

    if playlists.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled("  Loading...", Style::default().fg(th.dim))).block(block),
            area,
        );
        return;
    }

    if app.filtered_indices.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled("  No matches", Style::default().fg(th.dim))).block(block),
            area,
        );
        return;
    }

    let items: Vec<ListItem> = app
        .filtered_indices
        .iter()
        .filter_map(|&idx| {
            let name = playlists.get(idx)?;
            let is_current = !current.is_empty() && name == &current;
            Some(if is_current {
                ListItem::new(Line::from(vec![
                    Span::styled("♫ ", Style::default().fg(th.green)),
                    Span::styled(name.clone(), Style::default().fg(th.green)),
                ]))
            } else {
                ListItem::new(Line::from(vec![
                    Span::styled("  ", Style::default()),
                    Span::styled(name.clone(), Style::default().fg(th.text)),
                ]))
            })
        })
        .collect();

    let total = items.len();
    let inner_height = area.height.saturating_sub(2) as usize;

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(th.highlight_bg)
                .fg(th.accent)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");

    f.render_stateful_widget(list, area, &mut app.list_state);

    if total > inner_height {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .thumb_style(Style::default().fg(th.accent))
            .track_style(Style::default().fg(th.border));
        let mut scrollbar_state =
            ScrollbarState::new(total).position(app.list_state.selected().unwrap_or(0));
        let scroll_area = Rect {
            x: area.x,
            y: area.y + 1,
            width: area.width,
            height: area.height.saturating_sub(2),
        };
        f.render_stateful_widget(scrollbar, scroll_area, &mut scrollbar_state);
    }
}

fn draw_track_list(f: &mut Frame, area: Rect, app: &mut App) {
    let th = app.theme;
    let st = app.state.lock().unwrap();
    let tracks = st.playlist_tracks.clone();
    let now_playing = st.track.name.clone();
    let now_artist = st.track.artist.clone();
    drop(st);

    let playlist_name = if let BrowseView::Tracks(ref name) = app.browse_view {
        name.clone()
    } else {
        String::new()
    };

    let title = match &app.input_mode {
        InputMode::Search => {
            let blink = if (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                / 500)
                % 2
                == 0
            {
                "▏"
            } else {
                " "
            };
            format!(" Search: {}{} ", app.search_query, blink)
        }
        InputMode::Normal => format!(" {} ({}) ◂ Esc ", playlist_name, app.filtered_track_indices.len()),
    };

    let block = Block::default()
        .title(Span::styled(&title, Style::default().fg(th.text_dim).bold()))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(th.accent))
        .style(Style::default().bg(th.surface));

    if tracks.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled("  Loading tracks...", Style::default().fg(th.dim)))
                .block(block),
            area,
        );
        return;
    }

    if app.filtered_track_indices.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled("  No matches", Style::default().fg(th.dim))).block(block),
            area,
        );
        return;
    }

    let max_width = area.width.saturating_sub(8) as usize;

    let items: Vec<ListItem> = app
        .filtered_track_indices
        .iter()
        .filter_map(|&idx| {
            let t = tracks.get(idx)?;
            let is_playing = !now_playing.is_empty()
                && t.name == now_playing
                && t.artist == now_artist;
            let dur = format_time(t.duration);
            let prefix = if is_playing { "▶ " } else { "  " };
            let name_max = max_width.saturating_sub(dur.len() + prefix.len() + t.artist.chars().count() + 5);
            let name_display = if t.name.chars().count() > name_max {
                let limit = name_max.saturating_sub(1);
                let truncated: String = t.name.chars().take(limit).collect();
                format!("{}…", truncated)
            } else {
                t.name.clone()
            };

            let style = if is_playing {
                Style::default().fg(th.green)
            } else {
                Style::default().fg(th.text)
            };
            let dim_style = if is_playing {
                Style::default().fg(th.green)
            } else {
                Style::default().fg(th.text_dim)
            };

            Some(ListItem::new(Line::from(vec![
                Span::styled(prefix, style),
                Span::styled(name_display, style),
                Span::styled("  ", Style::default()),
                Span::styled(t.artist.clone(), dim_style),
                Span::styled(format!("  {}", dur), dim_style),
            ])))
        })
        .collect();

    let total = items.len();
    let inner_height = area.height.saturating_sub(2) as usize;

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(th.highlight_bg)
                .fg(th.accent)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");

    f.render_stateful_widget(list, area, &mut app.track_list_state);

    if total > inner_height {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .thumb_style(Style::default().fg(th.accent))
            .track_style(Style::default().fg(th.border));
        let mut scrollbar_state =
            ScrollbarState::new(total).position(app.track_list_state.selected().unwrap_or(0));
        let scroll_area = Rect {
            x: area.x,
            y: area.y + 1,
            width: area.width,
            height: area.height.saturating_sub(2),
        };
        f.render_stateful_widget(scrollbar, scroll_area, &mut scrollbar_state);
    }
}

fn draw_global_search(f: &mut Frame, area: Rect, app: &mut App) {
    let th = app.theme;
    let st = app.state.lock().unwrap();
    let now_playing = st.track.name.clone();
    let now_artist = st.track.artist.clone();
    drop(st);

    let title = match &app.input_mode {
        InputMode::Search => {
            let blink = if (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                / 500)
                % 2
                == 0
            {
                "▏"
            } else {
                " "
            };
            format!(" Library Search: {}{} ", app.global_search_query, blink)
        }
        InputMode::Normal => format!(
            " Library Search: {} ({}) ◂ Esc ",
            app.global_search_query,
            app.global_search_results.len()
        ),
    };

    let block = Block::default()
        .title(Span::styled(&title, Style::default().fg(th.text_dim).bold()))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(th.accent))
        .style(Style::default().bg(th.surface));

    if app.global_search_results.is_empty() {
        let msg = if app.global_search_query.is_empty() {
            "  Type to search your library..."
        } else {
            "  No results"
        };
        f.render_widget(
            Paragraph::new(Span::styled(msg, Style::default().fg(th.dim))).block(block),
            area,
        );
        return;
    }

    let max_width = area.width.saturating_sub(8) as usize;

    let items: Vec<ListItem> = app
        .global_search_results
        .iter()
        .map(|t| {
            let is_playing =
                !now_playing.is_empty() && t.name == now_playing && t.artist == now_artist;
            let dur = format_time(t.duration);
            let prefix = if is_playing { "▶ " } else { "  " };
            let name_max = max_width.saturating_sub(dur.len() + prefix.len() + t.artist.chars().count() + 5);
            let name_display = if t.name.chars().count() > name_max {
                let limit = name_max.saturating_sub(1);
                let truncated: String = t.name.chars().take(limit).collect();
                format!("{}…", truncated)
            } else {
                t.name.clone()
            };

            let style = if is_playing {
                Style::default().fg(th.green)
            } else {
                Style::default().fg(th.text)
            };
            let dim_style = if is_playing {
                Style::default().fg(th.green)
            } else {
                Style::default().fg(th.text_dim)
            };

            ListItem::new(Line::from(vec![
                Span::styled(prefix, style),
                Span::styled(name_display, style),
                Span::styled("  ", Style::default()),
                Span::styled(t.artist.clone(), dim_style),
                Span::styled(format!("  {}", dur), dim_style),
            ]))
        })
        .collect();

    let total = app.global_search_results.len();
    let inner_height = area.height.saturating_sub(2) as usize;

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(th.highlight_bg)
                .fg(th.accent)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");

    f.render_stateful_widget(list, area, &mut app.global_search_state);

    if total > inner_height {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .thumb_style(Style::default().fg(th.accent))
            .track_style(Style::default().fg(th.border));
        let mut scrollbar_state = ScrollbarState::new(total)
            .position(app.global_search_state.selected().unwrap_or(0));
        let scroll_area = Rect {
            x: area.x,
            y: area.y + 1,
            width: area.width,
            height: area.height.saturating_sub(2),
        };
        f.render_stateful_widget(scrollbar, scroll_area, &mut scrollbar_state);
    }
}

fn draw_right_panel(f: &mut Frame, area: Rect, app: &App) {
    let th = &app.theme;
    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(th.border))
        .style(Style::default().bg(th.surface));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(1), Constraint::Length(14)])
        .horizontal_margin(1)
        .vertical_margin(1)
        .split(inner);

    draw_up_next(f, chunks[0], app);
    draw_keyhints(f, chunks[2], app);
}

fn draw_up_next(f: &mut Frame, area: Rect, app: &App) {
    let th = &app.theme;
    let st = app.state.lock().unwrap();

    let mut lines = Vec::new();

    if !st.current_playlist.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("FROM ", Style::default().fg(th.dim).bold()),
            Span::styled(
                st.current_playlist.clone(),
                Style::default().fg(th.text).italic(),
            ),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "QUEUE",
        Style::default().fg(th.dim).bold(),
    )));

    if !st.queue.is_empty() {
        let max_width = area.width.saturating_sub(4) as usize;
        for (i, (name, artist)) in st.queue.iter().enumerate() {
            let num = format!("{:>2}. ", i + 1);
            let name_display = if name.chars().count() > max_width.saturating_sub(num.len()) {
                let limit = max_width.saturating_sub(num.len() + 1);
                let truncated: String = name.chars().take(limit).collect();
                format!("{}…", truncated)
            } else {
                name.clone()
            };
            lines.push(Line::from(vec![
                Span::styled(num, Style::default().fg(th.dim)),
                Span::styled(name_display, Style::default().fg(th.text)),
            ]));
            if !artist.is_empty() {
                lines.push(Line::from(Span::styled(
                    format!("    {}", artist),
                    Style::default().fg(th.text_dim),
                )));
            }
        }
    } else if !st.up_next_name.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(" 1. ", Style::default().fg(th.dim)),
            Span::styled(
                st.up_next_name.clone(),
                Style::default().fg(th.text),
            ),
        ]));
        if !st.up_next_artist.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("    {}", st.up_next_artist),
                Style::default().fg(th.text_dim),
            )));
        }
    } else {
        lines.push(Line::from(Span::styled("  ─", Style::default().fg(th.dim))));
    }

    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), area);
}

fn draw_keyhints(f: &mut Frame, area: Rect, app: &App) {
    let th = &app.theme;
    let hints: Vec<(&str, &str, bool)> = vec![
        ("SHORTCUTS", "", true),
        ("Space", "play / pause", false),
        ("n / p", "next / prev", false),
        ("+ / -", "volume", false),
        ("← / →", "seek ±10s", false),
        ("l", "love / unlove", false),
        ("x", "shuffle", false),
        ("v", "repeat", false),
        ("/", "search", false),
        ("Enter", "open / play", false),
        ("m", "mute", false),
        ("t", "theme", false),
        ("?", "help", false),
        ("q", "quit", false),
    ];

    let lines: Vec<Line> = hints
        .iter()
        .map(|(key, desc, is_header)| {
            if *is_header {
                Line::from(Span::styled(*key, Style::default().fg(th.dim).bold()))
            } else {
                Line::from(vec![
                    Span::styled(
                        format!("{:<8}", key),
                        Style::default().fg(th.accent).bold(),
                    ),
                    Span::styled(*desc, Style::default().fg(th.text_dim)),
                ])
            }
        })
        .collect();

    f.render_widget(Paragraph::new(lines), area);
}

fn draw_status_bar(f: &mut Frame, area: Rect, app: &App) {
    let th = &app.theme;
    let (icon, status) = {
        let st = app.state.lock().unwrap();
        (st.track.state.icon().to_string(), st.status.clone())
    };

    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(" {}  {} ", icon, status),
            Style::default().fg(th.text_dim),
        )))
        .style(Style::default().bg(th.surface_light)),
        area,
    );
}

fn draw_help_overlay(f: &mut Frame, area: Rect, theme: &Theme) {
    let th = theme;
    let width = 52.min(area.width.saturating_sub(4));
    let height = 30.min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(width)) / 2;
    let y = (area.height.saturating_sub(height)) / 2;
    let popup = Rect::new(x, y, width, height);

    f.render_widget(Clear, popup);

    let block = Block::default()
        .title(Span::styled(
            " Keyboard Shortcuts ",
            Style::default().fg(th.accent).bold(),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(th.accent))
        .style(Style::default().bg(th.surface));

    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let sections: Vec<(&str, Vec<(&str, &str)>)> = vec![
        (
            "PLAYBACK",
            vec![
                ("Space", "Play / Pause"),
                ("n  p", "Next / Previous"),
                ("s", "Stop"),
                ("l", "Love / Unlove track"),
                ("x", "Shuffle"),
                ("v", "Repeat (off → all → one)"),
            ],
        ),
        (
            "AUDIO",
            vec![
                ("+ / -", "Volume up / down (±5%)"),
                ("m", "Mute / Unmute"),
                ("← / →", "Seek back / forward 10s"),
            ],
        ),
        (
            "NAVIGATION",
            vec![
                ("j / k / ↑↓", "Move up / down"),
                ("Enter", "Open / Play selected"),
                ("Esc", "Back to playlists"),
                ("Tab", "Play whole playlist"),
                ("g / G", "Top / Bottom"),
                ("PgUp/Dn", "Page up / down"),
                ("/", "Search playlists"),
                ("F1", "Global library search"),
                ("F3", "Browse artists"),
            ],
        ),
        (
            "OTHER",
            vec![
                ("o", "Add to playlist"),
                ("h", "Recently played"),
                ("a", "AirPlay devices"),
                ("t", "Cycle theme"),
                ("r", "Refresh playlists"),
                ("F2", "Mini player"),
                ("?", "Toggle this help"),
                ("q", "Quit"),
            ],
        ),
    ];

    let mut lines: Vec<Line> = Vec::new();
    for (i, (title, keys)) in sections.iter().enumerate() {
        if i > 0 {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(
            *title,
            Style::default().fg(th.accent).bold(),
        )));
        for (key, desc) in keys {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {:<14}", key),
                    Style::default().fg(th.text).bold(),
                ),
                Span::styled(*desc, Style::default().fg(th.text_dim)),
            ]));
        }
    }

    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_airplay_overlay(f: &mut Frame, area: Rect, app: &mut App) {
    let th = app.theme;
    let width = 40.min(area.width.saturating_sub(4));
    let height = (app.airplay_devices.len() as u16 + 4).min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(width)) / 2;
    let y = (area.height.saturating_sub(height)) / 2;
    let popup = Rect::new(x, y, width, height);

    f.render_widget(Clear, popup);

    let block = Block::default()
        .title(Span::styled(
            " AirPlay Devices ",
            Style::default().fg(th.accent).bold(),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(th.accent))
        .style(Style::default().bg(th.surface));

    if app.airplay_devices.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled("  No devices found", Style::default().fg(th.dim)))
                .block(block),
            popup,
        );
        return;
    }

    let items: Vec<ListItem> = app
        .airplay_devices
        .iter()
        .map(|(name, selected)| {
            let icon = if *selected { "◉ " } else { "○ " };
            let color = if *selected { th.green } else { th.text };
            ListItem::new(Line::from(vec![
                Span::styled(icon, Style::default().fg(color)),
                Span::styled(name.clone(), Style::default().fg(color)),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(th.highlight_bg)
                .fg(th.accent)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");

    f.render_stateful_widget(list, popup, &mut app.airplay_list_state);
}

fn draw_add_to_playlist_overlay(f: &mut Frame, area: Rect, app: &mut App) {
    let th = app.theme;
    let st = app.state.lock().unwrap();
    let playlists = st.playlists.clone();
    drop(st);

    let width = 40.min(area.width.saturating_sub(4));
    let height = (playlists.len() as u16 + 4).min(area.height.saturating_sub(4)).max(5);
    let x = (area.width.saturating_sub(width)) / 2;
    let y = (area.height.saturating_sub(height)) / 2;
    let popup = Rect::new(x, y, width, height);

    f.render_widget(Clear, popup);

    let block = Block::default()
        .title(Span::styled(
            " Add to Playlist ",
            Style::default().fg(th.accent).bold(),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(th.accent))
        .style(Style::default().bg(th.surface));

    if playlists.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled("  No playlists found", Style::default().fg(th.dim)))
                .block(block),
            popup,
        );
        return;
    }

    let items: Vec<ListItem> = playlists
        .iter()
        .map(|name| {
            ListItem::new(Span::styled(
                format!("  {}", name),
                Style::default().fg(th.text),
            ))
        })
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(th.highlight_bg)
                .fg(th.accent)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");

    f.render_stateful_widget(list, popup, &mut app.add_to_playlist_state);
}

// ── Event handling ─────────────────────────────────────────────────

fn handle_key(app: &mut App, key: KeyEvent) {
    // Ctrl+C always force-quits regardless of mode
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        app.should_quit = true;
        return;
    }

    // F2 toggles mini mode from anywhere
    if key.code == KeyCode::F(2) {
        app.mini_mode = !app.mini_mode;
        app.set_status(if app.mini_mode { "Mini mode" } else { "Full mode" });
        return;
    }

    // AirPlay overlay handles its own keys
    if app.show_airplay {
        handle_airplay_key(app, key);
        return;
    }

    // Add-to-playlist overlay handles its own keys
    if app.show_add_to_playlist {
        handle_add_to_playlist_key(app, key);
        return;
    }

    match app.input_mode {
        InputMode::Search => handle_search_key(app, key),
        InputMode::Normal => handle_normal_key(app, key),
    }
}

fn handle_global_search_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            app.input_mode = InputMode::Normal;
            if app.global_search_results.is_empty() {
                app.browse_view = BrowseView::Playlists;
                app.update_filter();
            }
        }
        KeyCode::Enter => {
            app.input_mode = InputMode::Normal;
            // If we have a selected result, play it
            if let Some(sel) = app.global_search_state.selected() {
                if let Some(track) = app.global_search_results.get(sel) {
                    let name = track.name.clone();
                    let artist = track.artist.clone();
                    app.set_status(&format!("Playing: {}", name));
                    std::thread::spawn(move || cmd_play_library_track(&name, &artist));
                }
            } else if !app.global_search_query.is_empty() {
                // Execute the search
                let query = app.global_search_query.clone();
                app.set_status(&format!("Searching: {}", query));
                let results = fetch_library_search(&query, 50);
                app.set_status(&format!("Found {} results", results.len()));
                app.global_search_results = results;
                if !app.global_search_results.is_empty() {
                    app.global_search_state.select(Some(0));
                }
            }
        }
        KeyCode::Backspace => {
            app.global_search_query.pop();
        }
        KeyCode::Down => {
            let len = app.global_search_results.len();
            if len > 0 {
                let sel = app.global_search_state.selected().unwrap_or(0);
                app.global_search_state.select(Some((sel + 1).min(len - 1)));
            }
        }
        KeyCode::Up => {
            if let Some(sel) = app.global_search_state.selected() {
                app.global_search_state
                    .select(Some(sel.saturating_sub(1)));
            }
        }
        KeyCode::Char(c) => {
            app.global_search_query.push(c);
        }
        _ => {}
    }
}

fn handle_airplay_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('a') | KeyCode::Char('q') => {
            app.show_airplay = false;
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let len = app.airplay_devices.len();
            if len > 0 {
                let sel = app.airplay_list_state.selected().unwrap_or(0);
                app.airplay_list_state.select(Some((sel + 1).min(len - 1)));
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if let Some(sel) = app.airplay_list_state.selected() {
                app.airplay_list_state.select(Some(sel.saturating_sub(1)));
            }
        }
        KeyCode::Enter | KeyCode::Char(' ') => {
            if let Some(sel) = app.airplay_list_state.selected() {
                if let Some((name, _)) = app.airplay_devices.get(sel) {
                    let device_name = name.clone();
                    app.set_status(&format!("Toggling: {}", device_name));
                    std::thread::spawn(move || cmd_toggle_airplay(&device_name));
                    // Refresh devices after a short delay
                    let devices = fetch_airplay_devices();
                    app.airplay_devices = devices;
                }
            }
        }
        _ => {}
    }
}

fn handle_add_to_playlist_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('o') | KeyCode::Char('q') => {
            app.show_add_to_playlist = false;
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let len = app.state.lock().unwrap().playlists.len();
            if len > 0 {
                let sel = app.add_to_playlist_state.selected().unwrap_or(0);
                app.add_to_playlist_state.select(Some((sel + 1).min(len - 1)));
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if let Some(sel) = app.add_to_playlist_state.selected() {
                app.add_to_playlist_state.select(Some(sel.saturating_sub(1)));
            }
        }
        KeyCode::Enter => {
            if let Some(sel) = app.add_to_playlist_state.selected() {
                let st = app.state.lock().unwrap();
                if let Some(playlist_name) = st.playlists.get(sel).cloned() {
                    let track_name = st.track.name.clone();
                    drop(st);
                    app.set_status(&format!("Added \"{}\" to {}", track_name, playlist_name));
                    let pn = playlist_name.clone();
                    std::thread::spawn(move || cmd_add_to_playlist(&pn));
                    app.show_add_to_playlist = false;
                }
            }
        }
        _ => {}
    }
}

fn update_current_filter(app: &mut App) {
    match app.browse_view {
        BrowseView::Playlists => app.update_filter(),
        BrowseView::Tracks(_) => app.update_track_filter(),
        BrowseView::Artists => app.update_artist_filter(),
        BrowseView::ArtistTracks(_) => app.update_artist_track_filter(),
        BrowseView::GlobalSearch | BrowseView::RecentlyPlayed => {}
    }
}

fn handle_search_key(app: &mut App, key: KeyEvent) {
    if matches!(app.browse_view, BrowseView::GlobalSearch) {
        handle_global_search_key(app, key);
        return;
    }

    match key.code {
        KeyCode::Esc => {
            app.input_mode = InputMode::Normal;
            app.search_query.clear();
            update_current_filter(app);
        }
        KeyCode::Enter => {
            match app.browse_view {
                BrowseView::Tracks(_) => {
                    if let Some(track) = app.selected_track() {
                        if let BrowseView::Tracks(ref playlist) = app.browse_view {
                            let playlist = playlist.clone();
                            let track_name = track.name.clone();
                            let track_idx = track.index;
                            app.set_status(&format!("Playing: {}", track_name));
                            std::thread::spawn(move || {
                                cmd_play_track_in_playlist(&playlist, track_idx);
                            });
                        }
                    }
                }
                BrowseView::Artists => {
                    if let Some(sel) = app.artist_list_state.selected() {
                        if let Some(&idx) = app.filtered_artist_indices.get(sel) {
                            let st = app.state.lock().unwrap();
                            if let Some(artist_name) = st.artists.get(idx).cloned() {
                                drop(st);
                                let name_clone = artist_name.clone();
                                app.set_status(&format!("Loading: {}", artist_name));
                                app.browse_view = BrowseView::ArtistTracks(artist_name);
                                app.artist_track_list_state.select(Some(0));
                                let state = app.state.clone();
                                std::thread::spawn(move || {
                                    let tracks = fetch_artist_tracks(&name_clone);
                                    let mut st = state.lock().unwrap();
                                    st.status = format!("{} — {} tracks", name_clone, tracks.len());
                                    st.status_time = Instant::now();
                                    st.artist_tracks = tracks;
                                    st.dirty = true;
                                });
                            }
                        }
                    }
                }
                BrowseView::ArtistTracks(_) => {
                    if let Some(sel) = app.artist_track_list_state.selected() {
                        if let Some(&idx) = app.filtered_artist_track_indices.get(sel) {
                            let st = app.state.lock().unwrap();
                            if let Some(track) = st.artist_tracks.get(idx) {
                                let name = track.name.clone();
                                let track_artist = track.artist.clone();
                                drop(st);
                                app.set_status(&format!("Playing: {}", name));
                                std::thread::spawn(move || cmd_play_library_track(&name, &track_artist));
                            }
                        }
                    }
                }
                BrowseView::Playlists => {
                    if let Some(name) = app.selected_playlist_name() {
                        let name_clone = name.clone();
                        app.set_status(&format!("Loading tracks: {}", name));
                        app.browse_view = BrowseView::Tracks(name.clone());
                        app.track_list_state.select(Some(0));
                        let state = app.state.clone();
                        std::thread::spawn(move || {
                            let tracks = fetch_playlist_tracks(&name_clone);
                            let mut st = state.lock().unwrap();
                            st.status = format!("{} — {} tracks", name_clone, tracks.len());
                            st.status_time = Instant::now();
                            st.playlist_tracks = tracks;
                            st.dirty = true;
                        });
                    }
                }
                BrowseView::GlobalSearch | BrowseView::RecentlyPlayed => {}
            }
            app.input_mode = InputMode::Normal;
            app.search_query.clear();
            update_current_filter(app);
        }
        KeyCode::Backspace => {
            app.search_query.pop();
            update_current_filter(app);
        }
        KeyCode::Down => {
            let len = active_list_len(app);
            if len > 0 {
                let ls = active_list_state(app);
                let sel = ls.selected().unwrap_or(0);
                ls.select(Some((sel + 1).min(len - 1)));
            }
        }
        KeyCode::Up => {
            let ls = active_list_state(app);
            if let Some(sel) = ls.selected() {
                ls.select(Some(sel.saturating_sub(1)));
            }
        }
        KeyCode::Char(c) => {
            app.search_query.push(c);
            update_current_filter(app);
        }
        _ => {}
    }
}

fn active_list_len(app: &App) -> usize {
    match app.browse_view {
        BrowseView::Playlists => app.filtered_indices.len(),
        BrowseView::Tracks(_) => app.filtered_track_indices.len(),
        BrowseView::GlobalSearch => app.global_search_results.len(),
        BrowseView::Artists => app.filtered_artist_indices.len(),
        BrowseView::ArtistTracks(_) => app.filtered_artist_track_indices.len(),
        BrowseView::RecentlyPlayed => app.state.lock().unwrap().recent_tracks.len(),
    }
}

fn active_list_state(app: &mut App) -> &mut ListState {
    match app.browse_view {
        BrowseView::Playlists => &mut app.list_state,
        BrowseView::Tracks(_) => &mut app.track_list_state,
        BrowseView::GlobalSearch => &mut app.global_search_state,
        BrowseView::Artists => &mut app.artist_list_state,
        BrowseView::ArtistTracks(_) => &mut app.artist_track_list_state,
        BrowseView::RecentlyPlayed => &mut app.recent_list_state,
    }
}

fn handle_normal_key(app: &mut App, key: KeyEvent) {
    if app.show_help && key.code != KeyCode::Char('?') {
        app.show_help = false;
        return;
    }

    match key.code {
        KeyCode::Char('q') | KeyCode::Char('Q') => {
            match &app.browse_view {
                BrowseView::ArtistTracks(_) => {
                    app.browse_view = BrowseView::Artists;
                    app.search_query.clear();
                    app.update_artist_filter();
                }
                BrowseView::Tracks(_) | BrowseView::GlobalSearch | BrowseView::Artists | BrowseView::RecentlyPlayed => {
                    app.browse_view = BrowseView::Playlists;
                    app.search_query.clear();
                    app.update_filter();
                }
                BrowseView::Playlists => {
                    app.should_quit = true;
                }
            }
        }
        KeyCode::Char('?') => app.show_help = !app.show_help,
        KeyCode::Char(' ') => {
            app.set_status("Toggled play/pause");
            std::thread::spawn(cmd_play_pause);
        }
        KeyCode::Char('n') | KeyCode::Char('N') => {
            app.set_status("Next track");
            std::thread::spawn(cmd_next);
        }
        KeyCode::Char('p') | KeyCode::Char('P') => {
            app.set_status("Previous track");
            std::thread::spawn(cmd_prev);
        }
        KeyCode::Char('s') | KeyCode::Char('S') => {
            app.set_status("Stopped");
            std::thread::spawn(cmd_stop);
        }
        KeyCode::Char('l') | KeyCode::Char('L') => {
            let state = app.state.clone();
            std::thread::spawn(move || {
                cmd_toggle_love();
                // Fetch updated state
                let track = fetch_now_playing();
                let mut st = state.lock().unwrap();
                let loved = track.loved;
                st.track.loved = loved;
                st.status = match loved {
                    Some(true) => "♥ Loved".into(),
                    Some(false) => "♡ Unloved".into(),
                    None => "Love toggled".into(),
                };
                st.status_time = Instant::now();
            });
        }
        KeyCode::Char('x') | KeyCode::Char('X') => {
            let state = app.state.clone();
            std::thread::spawn(move || {
                cmd_toggle_shuffle();
                let shuf = fetch_shuffle();
                let mut st = state.lock().unwrap();
                st.shuffle = shuf;
                let msg = match shuf {
                    Some(true) => "Shuffle on",
                    Some(false) => "Shuffle off",
                    None => "Shuffle toggled",
                };
                st.status = msg.into();
                st.status_time = Instant::now();
            });
        }
        KeyCode::Char('v') => {
            let state = app.state.clone();
            std::thread::spawn(move || {
                let current = {
                    let st = state.lock().unwrap();
                    st.repeat_mode.clone().unwrap_or_else(|| "off".into())
                };
                let next = match current.as_str() {
                    "off" => "all",
                    "all" => "one",
                    _ => "off",
                };
                cmd_set_repeat(next);
                let mut st = state.lock().unwrap();
                st.repeat_mode = Some(next.into());
                st.status = format!("Repeat: {}", next);
                st.status_time = Instant::now();
            });
        }
        KeyCode::Char('+') | KeyCode::Char('=') => {
            let state = app.state.clone();
            std::thread::spawn(move || {
                let mut st = state.lock().unwrap();
                if st.volume < 0 {
                    drop(st);
                    let v = fetch_volume();
                    st = state.lock().unwrap();
                    st.volume = v;
                }
                let new = (st.volume + 5).min(100);
                st.volume = new;
                st.status = format!("Volume: {}%", new);
                st.status_time = Instant::now();
                drop(st);
                cmd_set_volume(new);
            });
        }
        KeyCode::Char('-') => {
            let state = app.state.clone();
            std::thread::spawn(move || {
                let mut st = state.lock().unwrap();
                if st.volume < 0 {
                    drop(st);
                    let v = fetch_volume();
                    st = state.lock().unwrap();
                    st.volume = v;
                }
                let new = (st.volume - 5).max(0);
                st.volume = new;
                st.status = format!("Volume: {}%", new);
                st.status_time = Instant::now();
                drop(st);
                cmd_set_volume(new);
            });
        }
        KeyCode::Char('m') | KeyCode::Char('M') => {
            let state = app.state.clone();
            std::thread::spawn(move || {
                let mut st = state.lock().unwrap();
                if st.volume < 0 {
                    drop(st);
                    let v = fetch_volume();
                    st = state.lock().unwrap();
                    st.volume = v;
                }
                let new = if st.volume > 0 {
                    st.pre_mute_vol = st.volume;
                    0
                } else {
                    st.pre_mute_vol
                };
                st.volume = new;
                st.status = if new == 0 {
                    "Muted".into()
                } else {
                    format!("Unmuted ({}%)", new)
                };
                st.status_time = Instant::now();
                drop(st);
                cmd_set_volume(new);
            });
        }
        KeyCode::Right => {
            let state = app.state.clone();
            std::thread::spawn(move || {
                let mut st = state.lock().unwrap();
                if st.track.state != PlayerState::Playing && st.track.state != PlayerState::Paused {
                    return;
                }
                let new_pos = (st.track.position + 10.0).min(st.track.duration);
                st.track.position = new_pos;
                st.last_position_time = Instant::now();
                st.status = "Seek forward 10s".into();
                st.status_time = Instant::now();
                drop(st);
                cmd_seek(new_pos);
            });
        }
        KeyCode::Left => {
            let state = app.state.clone();
            std::thread::spawn(move || {
                let mut st = state.lock().unwrap();
                if st.track.state != PlayerState::Playing && st.track.state != PlayerState::Paused {
                    return;
                }
                let new_pos = (st.track.position - 10.0).max(0.0);
                st.track.position = new_pos;
                st.last_position_time = Instant::now();
                st.status = "Seek back 10s".into();
                st.status_time = Instant::now();
                drop(st);
                cmd_seek(new_pos);
            });
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let len = active_list_len(app);
            if len > 0 {
                let ls = active_list_state(app);
                let sel = ls.selected().unwrap_or(0);
                ls.select(Some((sel + 1).min(len - 1)));
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            let ls = active_list_state(app);
            if let Some(sel) = ls.selected() {
                ls.select(Some(sel.saturating_sub(1)));
            }
        }
        KeyCode::Char('g') => {
            let len = active_list_len(app);
            if len > 0 {
                active_list_state(app).select(Some(0));
            }
        }
        KeyCode::Char('G') => {
            let len = active_list_len(app);
            if len > 0 {
                active_list_state(app).select(Some(len - 1));
            }
        }
        KeyCode::PageUp => {
            let ls = active_list_state(app);
            if let Some(sel) = ls.selected() {
                ls.select(Some(sel.saturating_sub(10)));
            }
        }
        KeyCode::PageDown => {
            let len = active_list_len(app);
            if len > 0 {
                let ls = active_list_state(app);
                let sel = ls.selected().unwrap_or(0);
                ls.select(Some((sel + 10).min(len - 1)));
            }
        }
        KeyCode::Esc => {
            match &app.browse_view {
                BrowseView::ArtistTracks(_) => {
                    app.browse_view = BrowseView::Artists;
                    app.search_query.clear();
                    app.update_artist_filter();
                }
                BrowseView::Tracks(_) | BrowseView::GlobalSearch | BrowseView::Artists | BrowseView::RecentlyPlayed => {
                    app.browse_view = BrowseView::Playlists;
                    app.search_query.clear();
                    app.update_filter();
                }
                _ => {}
            }
        }
        KeyCode::Enter => {
            match &app.browse_view {
                BrowseView::Playlists => {
                    if let Some(name) = app.selected_playlist_name() {
                        let name_clone = name.clone();
                        app.set_status(&format!("Loading tracks: {}", name));
                        app.browse_view = BrowseView::Tracks(name.clone());
                        app.track_list_state.select(Some(0));
                        app.search_query.clear();
                        let state = app.state.clone();
                        std::thread::spawn(move || {
                            let tracks = fetch_playlist_tracks(&name_clone);
                            let mut st = state.lock().unwrap();
                            st.status = format!("{} — {} tracks", name_clone, tracks.len());
                            st.status_time = Instant::now();
                            st.playlist_tracks = tracks;
                            st.dirty = true;
                        });
                    }
                }
                BrowseView::Tracks(playlist) => {
                    if let Some(track) = app.selected_track() {
                        let playlist = playlist.clone();
                        let track_name = track.name.clone();
                        let track_idx = track.index;
                        app.set_status(&format!("Playing: {}", track_name));
                        std::thread::spawn(move || {
                            cmd_play_track_in_playlist(&playlist, track_idx);
                        });
                    }
                }
                BrowseView::GlobalSearch => {
                    if let Some(sel) = app.global_search_state.selected() {
                        if let Some(track) = app.global_search_results.get(sel) {
                            let name = track.name.clone();
                            let artist = track.artist.clone();
                            app.set_status(&format!("Playing: {}", name));
                            std::thread::spawn(move || cmd_play_library_track(&name, &artist));
                        }
                    }
                }
                BrowseView::Artists => {
                    if let Some(sel) = app.artist_list_state.selected() {
                        let idx = app.filtered_artist_indices.get(sel).copied();
                        if let Some(idx) = idx {
                            let st = app.state.lock().unwrap();
                            if let Some(artist_name) = st.artists.get(idx).cloned() {
                                drop(st);
                                let name_clone = artist_name.clone();
                                app.set_status(&format!("Loading: {}", artist_name));
                                app.browse_view = BrowseView::ArtistTracks(artist_name);
                                app.artist_track_list_state.select(Some(0));
                                app.search_query.clear();
                                let state = app.state.clone();
                                std::thread::spawn(move || {
                                    let tracks = fetch_artist_tracks(&name_clone);
                                    let mut st = state.lock().unwrap();
                                    st.status = format!("{} — {} tracks", name_clone, tracks.len());
                                    st.status_time = Instant::now();
                                    st.artist_tracks = tracks;
                                    st.dirty = true;
                                });
                            }
                        }
                    }
                }
                BrowseView::ArtistTracks(_) => {
                    let sel = app.artist_track_list_state.selected();
                    if let Some(sel) = sel {
                        let idx = app.filtered_artist_track_indices.get(sel).copied();
                        if let Some(idx) = idx {
                            let st = app.state.lock().unwrap();
                            if let Some(track) = st.artist_tracks.get(idx) {
                                let name = track.name.clone();
                                let track_artist = track.artist.clone();
                                drop(st);
                                app.set_status(&format!("Playing: {}", name));
                                std::thread::spawn(move || cmd_play_library_track(&name, &track_artist));
                            }
                        }
                    }
                }
                BrowseView::RecentlyPlayed => {
                    if let Some(sel) = app.recent_list_state.selected() {
                        let st = app.state.lock().unwrap();
                        if let Some((name, artist)) = st.recent_tracks.get(sel).cloned() {
                            drop(st);
                            app.set_status(&format!("Playing: {}", name));
                            std::thread::spawn(move || cmd_play_library_track(&name, &artist));
                        }
                    }
                }
            }
        }
        KeyCode::Tab => {
            // Tab plays the whole playlist when in track view
            if let BrowseView::Tracks(ref playlist) = app.browse_view {
                let name = playlist.clone();
                app.set_status(&format!("Playing: {}", name));
                std::thread::spawn(move || cmd_play_playlist(&name));
            }
        }
        KeyCode::Char('/') => {
            if key.modifiers.contains(KeyModifiers::CONTROL) || matches!(app.browse_view, BrowseView::GlobalSearch) {
                // Enter global search mode
                app.browse_view = BrowseView::GlobalSearch;
                app.input_mode = InputMode::Search;
                app.global_search_query.clear();
                app.global_search_results.clear();
                app.global_search_state.select(None);
            } else {
                app.input_mode = InputMode::Search;
                app.search_query.clear();
            }
        }
        KeyCode::F(1) => {
            // F1 opens global search
            app.browse_view = BrowseView::GlobalSearch;
            app.input_mode = InputMode::Search;
            app.global_search_query.clear();
            app.global_search_results.clear();
            app.global_search_state.select(None);
        }
        KeyCode::F(3) => {
            // F3 opens artist browser
            let st = app.state.lock().unwrap();
            let has_artists = !st.artists.is_empty();
            drop(st);
            if has_artists {
                app.browse_view = BrowseView::Artists;
                app.search_query.clear();
                app.update_artist_filter();
            } else {
                app.set_status("Loading artists...");
                let state = app.state.clone();
                std::thread::spawn(move || {
                    let artists = fetch_artists();
                    let mut st = state.lock().unwrap();
                    st.status = format!("{} artists loaded", artists.len());
                    st.status_time = Instant::now();
                    st.artists = artists;
                    st.dirty = true;
                });
                app.browse_view = BrowseView::Artists;
            }
        }
        KeyCode::Char('r') | KeyCode::Char('R') => {
            let state = app.state.clone();
            app.set_status("Refreshing...");
            std::thread::spawn(move || {
                let playlists = fetch_playlists();
                let mut st = state.lock().unwrap();
                st.status = format!("Refreshed — {} playlists", playlists.len());
                st.status_time = Instant::now();
                st.playlists = playlists;
                st.dirty = true;
            });
        }
        KeyCode::Char('t') | KeyCode::Char('T') => {
            app.cycle_theme();
        }
        KeyCode::Char('a') | KeyCode::Char('A') => {
            app.airplay_devices = fetch_airplay_devices();
            app.airplay_list_state.select(Some(0));
            app.show_airplay = true;
        }
        KeyCode::Char('o') | KeyCode::Char('O') => {
            app.add_to_playlist_state.select(Some(0));
            app.show_add_to_playlist = true;
        }
        KeyCode::Char('h') | KeyCode::Char('H') => {
            app.browse_view = BrowseView::RecentlyPlayed;
            app.recent_list_state.select(Some(0));
        }
        KeyCode::F(2) => {
            app.mini_mode = !app.mini_mode;
            app.set_status(if app.mini_mode { "Mini mode" } else { "Full mode" });
        }
        _ => {}
    }
}

// ── Main ───────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> io::Result<()> {
    // Panic hook: restore terminal on crash so it doesn't stay in raw mode
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            LeaveAlternateScreen,
            crossterm::event::DisableMouseCapture
        );
        default_hook(info);
    }));

    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        crossterm::event::EnableMouseCapture
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let state = Arc::new(Mutex::new(AppState::default()));

    // Initial load
    {
        let playlists = fetch_playlists();
        let track = fetch_now_playing();
        let vol = fetch_volume();
        let shuf = fetch_shuffle();
        let rpt = fetch_repeat();
        let cp = fetch_current_playlist();
        let (un, ua) = fetch_up_next();
        let queue = fetch_queue(10);

        let mut st = state.lock().unwrap();
        st.playlists = playlists;
        st.last_track_name = track.name.clone();
        st.track = track;
        st.volume = vol;
        st.shuffle = shuf;
        st.repeat_mode = rpt;
        st.current_playlist = cp;
        st.up_next_name = un;
        st.up_next_artist = ua;
        st.queue = queue;
        st.last_position_time = Instant::now();
        st.status = format!("Loaded {} playlists", st.playlists.len());
        st.status_time = Instant::now();
    }

    // Background poller
    let poll_state = state.clone();
    let poll_handle = tokio::spawn(async move {
        let mut ticker = interval(POLL_INTERVAL);
        let mut poll_count = 0u32;
        loop {
            ticker.tick().await;
            poll_count += 1;

            let track = fetch_now_playing();
            let vol = fetch_volume();
            let shuf = fetch_shuffle();
            let rpt = fetch_repeat();
            let cp = fetch_current_playlist();
            let (un, ua) = fetch_up_next();
            let queue = fetch_queue(10);

            let mut st = poll_state.lock().unwrap();
            // Track recently played: if track changed, push the old one
            if !track.name.is_empty()
                && !st.last_track_name.is_empty()
                && track.name != st.last_track_name
            {
                let old_name = st.last_track_name.clone();
                let old_artist = st.track.artist.clone();
                // Avoid consecutive duplicates
                if st.recent_tracks.first().map(|(n, _)| n.as_str()) != Some(&old_name) {
                    st.recent_tracks.insert(0, (old_name, old_artist));
                    if st.recent_tracks.len() > 50 {
                        st.recent_tracks.truncate(50);
                    }
                }
            }
            st.last_track_name = track.name.clone();
            st.track = track;
            st.volume = vol;
            st.shuffle = shuf;
            st.repeat_mode = rpt;
            st.current_playlist = cp;
            st.up_next_name = un;
            st.up_next_artist = ua;
            st.queue = queue;
            st.last_position_time = Instant::now();

            if poll_count % 15 == 0 {
                drop(st);
                let playlists = fetch_playlists();
                let mut st = poll_state.lock().unwrap();
                st.playlists = playlists;
                st.dirty = true;
            }
        }
    });

    let mut app = App::new(state.clone());

    // Auto-scroll to the currently playing playlist
    {
        let st = state.lock().unwrap();
        let cp = st.current_playlist.clone();
        if !cp.is_empty() {
            if let Some(pos) = app.filtered_indices.iter().position(|&idx| {
                st.playlists.get(idx).map(|n| n.as_str()) == Some(&cp)
            }) {
                app.list_state.select(Some(pos));
            }
        }
    }

    let mut last_tick = Instant::now();

    loop {
        // Recalculate filters when background data changes
        {
            let mut st = state.lock().unwrap();
            if st.dirty {
                st.dirty = false;
                drop(st);
                let view_playlist = if let BrowseView::Tracks(p) = &app.browse_view {
                    Some(p.clone())
                } else {
                    None
                };
                match &app.browse_view {
                    BrowseView::Tracks(_) => {
                        app.update_track_filter();
                        // Auto-scroll to now-playing track if at default position
                        if app.track_list_state.selected() == Some(0) {
                            if let Some(ref vp) = view_playlist {
                                let st = state.lock().unwrap();
                                if st.current_playlist == *vp {
                                    let now_name = st.track.name.clone();
                                    drop(st);
                                    if !now_name.is_empty() {
                                        let st2 = app.state.lock().unwrap();
                                        if let Some(pos) = app.filtered_track_indices.iter().position(|&idx| {
                                            st2.playlist_tracks.get(idx).map(|t| t.name.as_str()) == Some(&now_name)
                                        }) {
                                            drop(st2);
                                            app.track_list_state.select(Some(pos));
                                        }
                                    }
                                }
                            }
                        }
                    }
                    BrowseView::Artists => app.update_artist_filter(),
                    BrowseView::ArtistTracks(_) => app.update_artist_track_filter(),
                    _ => app.update_filter(),
                }
            }
        }

        // Interpolate progress
        {
            let mut st = state.lock().unwrap();
            if st.track.state == PlayerState::Playing && st.track.duration > 0.0 {
                let now = Instant::now();
                let delta = now.duration_since(st.last_position_time).as_secs_f64();
                st.track.position = (st.track.position + delta).min(st.track.duration);
                st.last_position_time = now;
            }
            if st.status_time.elapsed() > Duration::from_secs(5) && st.status != "Ready" {
                st.status = "Ready".into();
            }
        }

        terminal.draw(|f| draw(f, &mut app))?;

        let timeout = TICK_RATE.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) => {
                    handle_key(&mut app, key);
                    if app.should_quit {
                        break;
                    }
                }
                Event::Mouse(MouseEvent {
                    kind: MouseEventKind::ScrollDown,
                    ..
                }) => {
                    if !app.show_airplay && !app.show_add_to_playlist && !app.show_help {
                        let len = active_list_len(&app);
                        if len > 0 {
                            let ls = active_list_state(&mut app);
                            let sel = ls.selected().unwrap_or(0);
                            ls.select(Some((sel + 3).min(len - 1)));
                        }
                    }
                }
                Event::Mouse(MouseEvent {
                    kind: MouseEventKind::ScrollUp,
                    ..
                }) => {
                    if !app.show_airplay && !app.show_add_to_playlist && !app.show_help {
                        let ls = active_list_state(&mut app);
                        if let Some(sel) = ls.selected() {
                            ls.select(Some(sel.saturating_sub(3)));
                        }
                    }
                }
                Event::Mouse(MouseEvent {
                    kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
                    column,
                    row,
                    ..
                }) => {
                    if let Some(bar_area) = app.progress_bar_area {
                        if row == bar_area.y
                            && column >= bar_area.x
                            && column < bar_area.x + bar_area.width
                        {
                            let st = app.state.lock().unwrap();
                            let duration = st.track.duration;
                            drop(st);
                            if duration > 0.0 {
                                let ratio =
                                    (column - bar_area.x) as f64 / bar_area.width as f64;
                                let new_pos = (ratio * duration).clamp(0.0, duration);
                                {
                                    let mut st = app.state.lock().unwrap();
                                    st.track.position = new_pos;
                                    st.last_position_time = Instant::now();
                                }
                                app.set_status(&format!(
                                    "Seek to {}",
                                    format_time(new_pos)
                                ));
                                std::thread::spawn(move || cmd_seek(new_pos));
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        if last_tick.elapsed() >= TICK_RATE {
            last_tick = Instant::now();
        }
    }

    poll_handle.abort();

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        crossterm::event::DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    Ok(())
}
