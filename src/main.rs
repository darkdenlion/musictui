use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, MouseEvent, MouseEventKind},
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

// ── Colors ─────────────────────────────────────────────────────────

const ACCENT: Color = Color::Rgb(100, 180, 255);
const GREEN: Color = Color::Rgb(80, 220, 130);
const YELLOW: Color = Color::Rgb(240, 200, 80);
const RED: Color = Color::Rgb(240, 90, 90);
const DIM: Color = Color::Rgb(100, 100, 115);
const SURFACE: Color = Color::Reset;
const SURFACE_LIGHT: Color = Color::Reset;
const TEXT: Color = Color::Rgb(220, 220, 230);
const TEXT_DIM: Color = Color::Rgb(140, 140, 155);
const BORDER: Color = Color::Rgb(55, 55, 70);
const HIGHLIGHT_BG: Color = Color::Rgb(60, 60, 80);

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

    fn color(&self) -> Color {
        match self {
            Self::Playing => GREEN,
            Self::Paused => YELLOW,
            _ => DIM,
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
}

impl App {
    fn new(state: Arc<Mutex<AppState>>) -> Self {
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
        };
        app.update_filter();
        app
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
    let script = format!(
        r#"tell application "{}"
    if it is running then return sound volume as string
end tell
return "-1""#,
        APP_NAME
    );

    match run_applescript(&script) {
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
    let script = format!(
        r#"tell application "{}"
    if it is running then
        if player state is stopped then return "NO"
        try
            set cp to current playlist
            set ct to current track
            set pid to persistent ID of ct
            set tl to tracks of cp
            set found to false
            set out to ""
            set cnt to 0
            repeat with i from 1 to count of tl
                if found then
                    set t to item i of tl
                    set out to out & name of t & "\t" & artist of t & "\n"
                    set cnt to cnt + 1
                    if cnt >= {} then exit repeat
                end if
                if persistent ID of item i of tl is pid then set found to true
            end repeat
            if out is not "" then return out
        end try
    end if
end tell
return "NO""#,
        APP_NAME, max_items
    );

    match run_applescript(&script) {
        Ok(out) => {
            if out == "NO" || out.is_empty() {
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
    let _ = run_applescript(&format!(
        r#"tell application "{}" to set sound volume to {}"#,
        APP_NAME, vol
    ));
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

fn cmd_set_repeat(mode: &str) {
    let _ = run_applescript(&format!(
        r#"tell application "{}" to set song repeat to {}"#,
        APP_NAME, mode
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

fn eq_frame() -> &'static str {
    const EQ_FRAMES: &[&str] = &[
        "▁▃▅▇▅▃", "▃▅▇▅▃▁", "▅▇▅▃▁▃", "▇▅▃▁▃▅", "▅▃▁▃▅▇", "▃▁▃▅▇▅",
    ];
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    EQ_FRAMES[(ms / 250) as usize % EQ_FRAMES.len()]
}

// ── Drawing ────────────────────────────────────────────────────────

fn draw(f: &mut Frame, app: &mut App) {
    let size = f.area();
    f.render_widget(Block::default().style(Style::default().bg(SURFACE)), size);

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
        draw_help_overlay(f, size);
    }
}

fn draw_header(f: &mut Frame, area: Rect, app: &App) {
    let st = app.state.lock().unwrap();
    let state_str = match st.track.state {
        PlayerState::Playing => format!(" {} playing ", eq_frame()),
        PlayerState::Paused => " ⏸ paused ".into(),
        PlayerState::Stopped => " ⏹ stopped ".into(),
        PlayerState::NotRunning => " ○ not running ".into(),
    };
    let state_color = st.track.state.color();
    drop(st);

    let title = " ♫ Apple Music ";
    let pad_len = area
        .width
        .saturating_sub(title.len() as u16 + state_str.len() as u16) as usize;

    let header = Line::from(vec![
        Span::styled(title, Style::default().fg(ACCENT).bold()),
        Span::styled(" ".repeat(pad_len), Style::default().bg(SURFACE_LIGHT)),
        Span::styled(state_str, Style::default().fg(state_color).bold()),
    ]);

    f.render_widget(
        Paragraph::new(header).style(Style::default().bg(SURFACE_LIGHT)),
        area,
    );
}

fn draw_body(f: &mut Frame, area: Rect, app: &mut App) {
    let body_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(40), Constraint::Length(32)])
        .split(area);

    draw_left_panel(f, body_chunks[0], app);
    draw_right_panel(f, body_chunks[1], app);
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
    let st = app.state.lock().unwrap();
    let t = &st.track;

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(BORDER))
        .style(Style::default().bg(SURFACE));

    let inner = block.inner(area);
    f.render_widget(block, area);

    if t.state == PlayerState::NotRunning || t.state == PlayerState::Stopped {
        let msg = if t.state == PlayerState::NotRunning {
            "Music app is not running"
        } else {
            "Nothing playing"
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(msg, Style::default().fg(DIM)))),
            inner,
        );
        return;
    }

    let mut title_spans = vec![Span::styled(
        t.name.clone(),
        Style::default().fg(TEXT).bold(),
    )];
    if t.loved == Some(true) {
        title_spans.push(Span::styled("  ♥", Style::default().fg(RED)));
    }
    let title = Line::from(title_spans);
    let subtitle = Line::from(vec![
        Span::styled(t.artist.clone(), Style::default().fg(TEXT_DIM)),
        Span::styled("  ·  ", Style::default().fg(DIM)),
        Span::styled(t.album.clone(), Style::default().fg(TEXT_DIM)),
    ]);
    let state_line = Line::from(Span::styled(
        format!("{} {}", t.state.icon(), t.state.label()),
        Style::default().fg(t.state.color()),
    ));

    f.render_widget(
        Paragraph::new(vec![title, subtitle, Line::from(""), state_line]),
        inner,
    );
}

fn draw_progress_bar(f: &mut Frame, area: Rect, app: &App) {
    let st = app.state.lock().unwrap();
    let t = &st.track;

    if t.duration <= 0.0 {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  ╌╌╌ no track ╌╌╌",
                Style::default().fg(DIM),
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
        Span::styled(format!("  {} ", pos_str), Style::default().fg(TEXT_DIM)),
        Span::styled("━".repeat(filled), Style::default().fg(ACCENT)),
        Span::styled("●", Style::default().fg(TEXT).bold()),
        Span::styled("╌".repeat(empty), Style::default().fg(DIM)),
        Span::styled(format!(" {} ", dur_str), Style::default().fg(TEXT_DIM)),
    ]);

    f.render_widget(Paragraph::new(bar), area);
}

fn draw_controls(f: &mut Frame, area: Rect, app: &App) {
    let st = app.state.lock().unwrap();

    let mut spans = Vec::new();
    spans.push(Span::styled("  ", Style::default()));

    match st.shuffle {
        Some(true) => spans.push(Span::styled("⇆ On ", Style::default().fg(GREEN).bold())),
        Some(false) => spans.push(Span::styled("⇆ Off ", Style::default().fg(DIM))),
        None => spans.push(Span::styled("⇆ ─ ", Style::default().fg(DIM))),
    }

    spans.push(Span::styled("   ", Style::default()));

    match st.repeat_mode.as_deref() {
        Some("all") => spans.push(Span::styled("↻ All ", Style::default().fg(GREEN).bold())),
        Some("one") => spans.push(Span::styled("↻ One ", Style::default().fg(YELLOW).bold())),
        _ => spans.push(Span::styled("↻ Off ", Style::default().fg(DIM))),
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
            RED
        } else if st.volume < 30 {
            TEXT_DIM
        } else if st.volume < 70 {
            ACCENT
        } else {
            GREEN
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
            Style::default().fg(DIM),
        ));
        spans.push(Span::styled(
            format!(" {}%", st.volume),
            Style::default().fg(TEXT_DIM),
        ));
    }

    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_playlist(f: &mut Frame, area: Rect, app: &mut App) {
    match &app.browse_view {
        BrowseView::Playlists => draw_playlist_list(f, area, app),
        BrowseView::Tracks(_) => draw_track_list(f, area, app),
    }
}

fn draw_playlist_list(f: &mut Frame, area: Rect, app: &mut App) {
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
        InputMode::Search => ACCENT,
        InputMode::Normal => BORDER,
    };

    let block = Block::default()
        .title(Span::styled(&title, Style::default().fg(TEXT_DIM).bold()))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .style(Style::default().bg(SURFACE));

    if playlists.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled("  Loading...", Style::default().fg(DIM))).block(block),
            area,
        );
        return;
    }

    if app.filtered_indices.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled("  No matches", Style::default().fg(DIM))).block(block),
            area,
        );
        return;
    }

    let items: Vec<ListItem> = app
        .filtered_indices
        .iter()
        .map(|&idx| {
            let name = &playlists[idx];
            let is_current = !current.is_empty() && name == &current;
            if is_current {
                ListItem::new(Line::from(vec![
                    Span::styled("♫ ", Style::default().fg(GREEN)),
                    Span::styled(name.clone(), Style::default().fg(GREEN)),
                ]))
            } else {
                ListItem::new(Line::from(vec![
                    Span::styled("  ", Style::default()),
                    Span::styled(name.clone(), Style::default().fg(TEXT)),
                ]))
            }
        })
        .collect();

    let total = app.filtered_indices.len();
    let inner_height = area.height.saturating_sub(2) as usize;

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(HIGHLIGHT_BG)
                .fg(ACCENT)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");

    f.render_stateful_widget(list, area, &mut app.list_state);

    if total > inner_height {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .thumb_style(Style::default().fg(ACCENT))
            .track_style(Style::default().fg(BORDER));
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

    let border_color = match &app.input_mode {
        InputMode::Search => ACCENT,
        InputMode::Normal => ACCENT,
    };

    let block = Block::default()
        .title(Span::styled(&title, Style::default().fg(TEXT_DIM).bold()))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .style(Style::default().bg(SURFACE));

    if tracks.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled("  Loading tracks...", Style::default().fg(DIM)))
                .block(block),
            area,
        );
        return;
    }

    if app.filtered_track_indices.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled("  No matches", Style::default().fg(DIM))).block(block),
            area,
        );
        return;
    }

    let max_width = area.width.saturating_sub(8) as usize;

    let items: Vec<ListItem> = app
        .filtered_track_indices
        .iter()
        .map(|&idx| {
            let t = &tracks[idx];
            let is_playing = !now_playing.is_empty()
                && t.name == now_playing
                && t.artist == now_artist;
            let dur = format_time(t.duration);
            let prefix = if is_playing { "▶ " } else { "  " };
            let name_max = max_width.saturating_sub(dur.len() + prefix.len() + t.artist.len() + 5);
            let name_display = if t.name.len() > name_max {
                format!("{}…", &t.name[..name_max.saturating_sub(1)])
            } else {
                t.name.clone()
            };

            let style = if is_playing {
                Style::default().fg(GREEN)
            } else {
                Style::default().fg(TEXT)
            };
            let dim_style = if is_playing {
                Style::default().fg(GREEN)
            } else {
                Style::default().fg(TEXT_DIM)
            };

            ListItem::new(Line::from(vec![
                Span::styled(prefix, style),
                Span::styled(name_display, style),
                Span::styled("  ", Style::default()),
                Span::styled(&t.artist, dim_style),
                Span::styled(format!("  {}", dur), dim_style),
            ]))
        })
        .collect();

    let total = app.filtered_track_indices.len();
    let inner_height = area.height.saturating_sub(2) as usize;

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(HIGHLIGHT_BG)
                .fg(ACCENT)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");

    f.render_stateful_widget(list, area, &mut app.track_list_state);

    if total > inner_height {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .thumb_style(Style::default().fg(ACCENT))
            .track_style(Style::default().fg(BORDER));
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

fn draw_right_panel(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(BORDER))
        .style(Style::default().bg(SURFACE));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(1), Constraint::Length(14)])
        .horizontal_margin(1)
        .vertical_margin(1)
        .split(inner);

    draw_up_next(f, chunks[0], app);
    draw_keyhints(f, chunks[2]);
}

fn draw_up_next(f: &mut Frame, area: Rect, app: &App) {
    let st = app.state.lock().unwrap();

    let mut lines = Vec::new();

    // Playing from
    if !st.current_playlist.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("FROM ", Style::default().fg(DIM).bold()),
            Span::styled(
                st.current_playlist.clone(),
                Style::default().fg(TEXT).italic(),
            ),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "QUEUE",
        Style::default().fg(DIM).bold(),
    )));

    if !st.queue.is_empty() {
        let max_width = area.width.saturating_sub(4) as usize;
        for (i, (name, artist)) in st.queue.iter().enumerate() {
            let num = format!("{:>2}. ", i + 1);
            let name_display = if name.len() > max_width.saturating_sub(num.len()) {
                format!(
                    "{}…",
                    &name[..max_width.saturating_sub(num.len() + 1)]
                )
            } else {
                name.clone()
            };
            lines.push(Line::from(vec![
                Span::styled(num, Style::default().fg(DIM)),
                Span::styled(name_display, Style::default().fg(TEXT)),
            ]));
            if !artist.is_empty() {
                lines.push(Line::from(Span::styled(
                    format!("    {}", artist),
                    Style::default().fg(TEXT_DIM),
                )));
            }
        }
    } else if !st.up_next_name.is_empty() {
        // Fallback to single up next
        lines.push(Line::from(vec![
            Span::styled(" 1. ", Style::default().fg(DIM)),
            Span::styled(
                st.up_next_name.clone(),
                Style::default().fg(TEXT),
            ),
        ]));
        if !st.up_next_artist.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("    {}", st.up_next_artist),
                Style::default().fg(TEXT_DIM),
            )));
        }
    } else {
        lines.push(Line::from(Span::styled("  ─", Style::default().fg(DIM))));
    }

    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), area);
}

fn draw_keyhints(f: &mut Frame, area: Rect) {
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
        ("?", "help", false),
        ("q", "quit", false),
    ];

    let lines: Vec<Line> = hints
        .iter()
        .map(|(key, desc, is_header)| {
            if *is_header {
                Line::from(Span::styled(*key, Style::default().fg(DIM).bold()))
            } else {
                Line::from(vec![
                    Span::styled(
                        format!("{:<8}", key),
                        Style::default().fg(ACCENT).bold(),
                    ),
                    Span::styled(*desc, Style::default().fg(TEXT_DIM)),
                ])
            }
        })
        .collect();

    f.render_widget(Paragraph::new(lines), area);
}

fn draw_status_bar(f: &mut Frame, area: Rect, app: &App) {
    let (icon, status) = {
        let st = app.state.lock().unwrap();
        (st.track.state.icon().to_string(), st.status.clone())
    };

    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(" {}  {} ", icon, status),
            Style::default().fg(TEXT_DIM),
        )))
        .style(Style::default().bg(SURFACE_LIGHT)),
        area,
    );
}

fn draw_help_overlay(f: &mut Frame, area: Rect) {
    let width = 52.min(area.width.saturating_sub(4));
    let height = 26.min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(width)) / 2;
    let y = (area.height.saturating_sub(height)) / 2;
    let popup = Rect::new(x, y, width, height);

    f.render_widget(Clear, popup);

    let block = Block::default()
        .title(Span::styled(
            " Keyboard Shortcuts ",
            Style::default().fg(ACCENT).bold(),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(ACCENT))
        .style(Style::default().bg(SURFACE));

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
                ("Esc", "Cancel search"),
            ],
        ),
        (
            "OTHER",
            vec![
                ("r", "Refresh playlists"),
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
            Style::default().fg(ACCENT).bold(),
        )));
        for (key, desc) in keys {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {:<14}", key),
                    Style::default().fg(TEXT).bold(),
                ),
                Span::styled(*desc, Style::default().fg(TEXT_DIM)),
            ]));
        }
    }

    f.render_widget(Paragraph::new(lines), inner);
}

// ── Event handling ─────────────────────────────────────────────────

fn handle_key(app: &mut App, key: KeyEvent) {
    match app.input_mode {
        InputMode::Search => handle_search_key(app, key),
        InputMode::Normal => handle_normal_key(app, key),
    }
}

fn handle_search_key(app: &mut App, key: KeyEvent) {
    let in_tracks = matches!(app.browse_view, BrowseView::Tracks(_));

    match key.code {
        KeyCode::Esc => {
            app.input_mode = InputMode::Normal;
            app.search_query.clear();
            if in_tracks {
                app.update_track_filter();
            } else {
                app.update_filter();
            }
        }
        KeyCode::Enter => {
            if in_tracks {
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
            } else if let Some(name) = app.selected_playlist_name() {
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
                });
            }
            app.input_mode = InputMode::Normal;
            app.search_query.clear();
            if in_tracks {
                app.update_track_filter();
            } else {
                app.update_filter();
            }
        }
        KeyCode::Backspace => {
            app.search_query.pop();
            if in_tracks {
                app.update_track_filter();
            } else {
                app.update_filter();
            }
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
            if in_tracks {
                app.update_track_filter();
            } else {
                app.update_filter();
            }
        }
        _ => {}
    }
}

fn active_list_len(app: &App) -> usize {
    match app.browse_view {
        BrowseView::Playlists => app.filtered_indices.len(),
        BrowseView::Tracks(_) => app.filtered_track_indices.len(),
    }
}

fn active_list_state(app: &mut App) -> &mut ListState {
    match app.browse_view {
        BrowseView::Playlists => &mut app.list_state,
        BrowseView::Tracks(_) => &mut app.track_list_state,
    }
}

fn handle_normal_key(app: &mut App, key: KeyEvent) {
    if app.show_help && key.code != KeyCode::Char('?') {
        app.show_help = false;
        return;
    }

    match key.code {
        KeyCode::Char('q') | KeyCode::Char('Q') => {
            if matches!(app.browse_view, BrowseView::Tracks(_)) {
                app.browse_view = BrowseView::Playlists;
                app.search_query.clear();
                app.update_filter();
            } else {
                app.should_quit = true;
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
            if matches!(app.browse_view, BrowseView::Tracks(_)) {
                app.browse_view = BrowseView::Playlists;
                app.search_query.clear();
                app.update_filter();
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
            app.input_mode = InputMode::Search;
            app.search_query.clear();
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
            });
        }
        _ => {}
    }
}

// ── Main ───────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> io::Result<()> {
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
            }
        }
    });

    let mut app = App::new(state.clone());

    let mut last_tick = Instant::now();

    loop {
        app.update_filter();
        if matches!(app.browse_view, BrowseView::Tracks(_)) {
            app.update_track_filter();
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
                    let len = active_list_len(&app);
                    if len > 0 {
                        let ls = active_list_state(&mut app);
                        let sel = ls.selected().unwrap_or(0);
                        ls.select(Some((sel + 3).min(len - 1)));
                    }
                }
                Event::Mouse(MouseEvent {
                    kind: MouseEventKind::ScrollUp,
                    ..
                }) => {
                    let ls = active_list_state(&mut app);
                    if let Some(sel) = ls.selected() {
                        ls.select(Some(sel.saturating_sub(3)));
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
