#!/usr/bin/env python3
"""Apple Music TUI — a modern terminal controller for Apple Music on macOS."""

import subprocess
import time
import threading
from dataclasses import dataclass, field
from typing import Optional

from textual import on, work
from textual.app import App, ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal, Vertical, Container
from textual.css.query import NoMatches
from textual.reactive import reactive
from textual.timer import Timer
from textual.widgets import (
    Header,
    Footer,
    Static,
    ListView,
    ListItem,
    Label,
    ProgressBar,
    Input,
)

# ── Constants ─────────────────────────────────────────────────────────

APP_NAME = "Music"
APPLESCRIPT_TIMEOUT = 5.0
POLL_INTERVAL = 2.0

# ── AppleScript helpers ──────────────────────────────────────────────


def run_applescript(script: str, timeout: float = APPLESCRIPT_TIMEOUT) -> tuple[str, str, int]:
    proc = subprocess.Popen(
        ["/usr/bin/osascript", "-e", script],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    try:
        out, err = proc.communicate(timeout=timeout)
        return out.strip(), err.strip(), proc.returncode
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait()
        return "", "AppleScript timed out", -1


def applescript_escape(value: str) -> str:
    escaped = value.replace("\\", "\\\\").replace('"', '\\"')
    return escaped.replace("\n", " ").replace("\r", " ")


def parse_number(raw: str) -> float:
    if not raw:
        return 0.0
    text = raw.strip()
    if "," in text and "." not in text:
        text = text.replace(",", ".")
    else:
        text = text.replace(",", "")
    filtered = "".join(ch for ch in text if ch.isdigit() or ch in ".-")
    if not filtered:
        return 0.0
    try:
        return float(filtered)
    except ValueError:
        return 0.0


def format_time(seconds: float) -> str:
    if seconds <= 0:
        return "0:00"
    total = int(seconds)
    m, s = divmod(total, 60)
    return f"{m}:{s:02d}"


def format_error(err: str) -> str:
    if not err:
        return ""
    low = err.lower()
    if "not authorized" in low or "not permitted" in low:
        return "Permission denied — enable Automation in System Settings > Privacy & Security"
    return err


# ── Data ─────────────────────────────────────────────────────────────


@dataclass
class TrackInfo:
    name: str = ""
    artist: str = ""
    album: str = ""
    state: str = "STOPPED"
    duration: float = 0.0
    position: float = 0.0


@dataclass
class MusicState:
    track: TrackInfo = field(default_factory=TrackInfo)
    playlists: list[str] = field(default_factory=list)
    up_next_name: str = ""
    up_next_artist: str = ""
    shuffle: Optional[bool] = None
    repeat_mode: Optional[str] = None
    volume: int = -1
    current_playlist: str = ""
    last_position_time: float = 0.0
    _pre_mute_vol: int = 50


# ── AppleScript commands ─────────────────────────────────────────────


def fetch_now_playing() -> TrackInfo:
    script = f'''
    tell application "{APP_NAME}"
        if it is running then
            if player state is stopped then return "STOPPED"
            set t to current track
            return name of t & "\\n" & artist of t & "\\n" & album of t & "\\n" & (player state as string) & "\\n" & duration of t & "\\n" & player position
        end if
    end tell
    return "NOT_RUNNING"
    '''
    out, err, code = run_applescript(script)
    if err or code != 0 or out in ("STOPPED", "NOT_RUNNING", ""):
        return TrackInfo(state=out if out else "STOPPED")
    parts = out.split("\n")
    if len(parts) >= 6:
        return TrackInfo(
            name=parts[0], artist=parts[1], album=parts[2],
            state=parts[3].upper(),
            duration=parse_number(parts[4]),
            position=parse_number(parts[5]),
        )
    return TrackInfo(state="STOPPED")


def fetch_playlists() -> list[str]:
    script = f'''
    set AppleScript's text item delimiters to "\\n"
    tell application "{APP_NAME}"
        if it is running then return name of playlists as text
    end tell
    return "NOT_RUNNING"
    '''
    out, err, code = run_applescript(script)
    if err or code != 0 or out in ("NOT_RUNNING", ""):
        return []
    return [p.strip() for p in out.split("\n") if p.strip()]


def fetch_up_next_ui() -> tuple[str, str]:
    script = f'''
    tell application "System Events"
        if not (exists process "{APP_NAME}") then return "NO"
        tell process "{APP_NAME}"
            if not (exists window 1) then return "NO"
            try
                set theTable to first table of scroll area 1 of window 1
                set row1 to first row of theTable
                set texts to value of static text of row1
                if (count of texts) >= 2 then
                    return item 1 of texts & "\\n" & item 2 of texts
                else if (count of texts) = 1 then
                    return item 1 of texts
                end if
            end try
        end tell
    end tell
    return "NO"
    '''
    out, _, code = run_applescript(script)
    if code != 0 or out in ("NO", ""):
        return "", ""
    parts = out.split("\n")
    return parts[0] if parts else "", parts[1] if len(parts) > 1 else ""


def fetch_up_next_playlist() -> tuple[str, str]:
    script = f'''
    tell application "{APP_NAME}"
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
                            return name of nt & "\\n" & artist of nt
                        end if
                    end if
                end repeat
            end try
        end if
    end tell
    return "NO"
    '''
    out, _, code = run_applescript(script)
    if code != 0 or out in ("NO", ""):
        return "", ""
    parts = out.split("\n")
    return parts[0] if parts else "", parts[1] if len(parts) > 1 else ""


def fetch_shuffle() -> Optional[bool]:
    script = f'''
    tell application "{APP_NAME}"
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
    return "UNKNOWN"
    '''
    out, _, code = run_applescript(script)
    if code != 0:
        return None
    if out == "true":
        return True
    if out == "false":
        return False
    return None


def fetch_repeat() -> Optional[str]:
    script = f'''
    tell application "{APP_NAME}"
        if it is running then
            try
                return song repeat as string
            end try
        end if
    end tell
    return "UNKNOWN"
    '''
    out, _, code = run_applescript(script)
    if code != 0:
        return None
    low = out.strip().lower()
    if low in ("off", "none"):
        return "off"
    if low == "one":
        return "one"
    if low == "all":
        return "all"
    return None


def fetch_volume() -> int:
    script = f'''
    tell application "{APP_NAME}"
        if it is running then return sound volume as string
    end tell
    return "-1"
    '''
    out, _, code = run_applescript(script)
    if code != 0:
        return -1
    try:
        return int(parse_number(out))
    except (ValueError, TypeError):
        return -1


def fetch_current_playlist() -> str:
    script = f'''
    tell application "{APP_NAME}"
        if it is running then
            if player state is not stopped then
                try
                    return name of current playlist
                end try
            end if
        end if
    end tell
    return ""
    '''
    out, _, code = run_applescript(script)
    return out.strip() if code == 0 else ""


def cmd_play_pause():
    run_applescript(f'tell application "{APP_NAME}" to playpause')


def cmd_next():
    run_applescript(f'tell application "{APP_NAME}" to next track')


def cmd_prev():
    run_applescript(f'tell application "{APP_NAME}" to previous track')


def cmd_stop():
    run_applescript(f'tell application "{APP_NAME}" to stop')


def cmd_play_playlist(name: str):
    safe = applescript_escape(name)
    run_applescript(f'tell application "{APP_NAME}" to play playlist "{safe}"')


def cmd_set_volume(vol: int):
    run_applescript(f'tell application "{APP_NAME}" to set sound volume to {vol}')


def cmd_seek(pos: float):
    run_applescript(f'tell application "{APP_NAME}" to set player position to {pos}')


def cmd_toggle_shuffle():
    script = f'''
    tell application "{APP_NAME}"
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
    end tell
    '''
    run_applescript(script)


def cmd_set_repeat(mode: str):
    run_applescript(f'tell application "{APP_NAME}" to set song repeat to {mode}')


# ── Widgets ──────────────────────────────────────────────────────────


class NowPlaying(Static):
    """Displays the currently playing track info."""

    track: reactive[TrackInfo] = reactive(TrackInfo, recompose=True)

    def render(self) -> str:
        t = self.track
        if t.state in ("NOT_RUNNING", "STOPPED", "UNKNOWN", ""):
            return "[dim]Nothing playing[/dim]"

        state_icon = {"PLAYING": "▶", "PAUSED": "⏸"}.get(t.state, "⏹")
        state_color = {"PLAYING": "green", "PAUSED": "yellow"}.get(t.state, "white")

        lines = []
        lines.append(f"[bold white]{t.name or 'Untitled'}[/]")
        lines.append(f"[dim]{t.artist or 'Unknown'}  ·  {t.album or 'Unknown'}[/]")
        lines.append(f"[{state_color}]{state_icon} {t.state.capitalize()}[/]")
        return "\n".join(lines)


class TrackProgress(Static):
    """Progress bar for the current track."""

    position: reactive[float] = reactive(0.0)
    duration: reactive[float] = reactive(0.0)

    def render(self) -> str:
        if self.duration <= 0:
            return "[dim]─── no track ───[/dim]"
        ratio = max(0.0, min(1.0, self.position / self.duration))
        width = 40
        filled = int(ratio * width)
        bar = "[bold cyan]━[/]" * filled + "[dim]╌[/]" * (width - filled)
        pos_str = format_time(self.position)
        dur_str = format_time(self.duration)
        return f"  {pos_str}  {bar}  {dur_str}"


class PlayerControls(Static):
    """Shows shuffle, repeat, volume status chips."""

    shuffle: reactive[Optional[bool]] = reactive(None)
    repeat_mode: reactive[Optional[str]] = reactive(None)
    volume: reactive[int] = reactive(-1)

    def render(self) -> str:
        parts = []

        # Shuffle
        if self.shuffle is True:
            parts.append("[bold green]⇆ Shuffle[/]")
        elif self.shuffle is False:
            parts.append("[dim]⇆ Shuffle[/]")
        else:
            parts.append("[dim]⇆ ─[/]")

        # Repeat
        rpt = self.repeat_mode or "off"
        if rpt == "all":
            parts.append("[bold green]↻ All[/]")
        elif rpt == "one":
            parts.append("[bold yellow]↻ One[/]")
        else:
            parts.append("[dim]↻ Off[/]")

        # Volume
        if self.volume >= 0:
            if self.volume == 0:
                parts.append(f"[bold red]🔇 {self.volume}%[/]")
            elif self.volume < 30:
                parts.append(f"[dim]♪ {self.volume}%[/]")
            elif self.volume < 70:
                parts.append(f"♪ {self.volume}%")
            else:
                parts.append(f"[bold]♪ {self.volume}%[/]")

        return "    ".join(parts)


class UpNextPanel(Static):
    """Shows the up next track and current playlist."""

    up_next_name: reactive[str] = reactive("")
    up_next_artist: reactive[str] = reactive("")
    current_playlist: reactive[str] = reactive("")

    def render(self) -> str:
        lines = []
        lines.append("[bold dim]UP NEXT[/]")
        if self.up_next_name:
            lines.append(f"  [white]{self.up_next_name}[/]")
            if self.up_next_artist:
                lines.append(f"  [dim]{self.up_next_artist}[/]")
        else:
            lines.append("  [dim]─[/]")

        lines.append("")
        lines.append("[bold dim]PLAYING FROM[/]")
        if self.current_playlist:
            lines.append(f"  [italic]{self.current_playlist}[/]")
        else:
            lines.append("  [dim]─[/]")

        return "\n".join(lines)


class PlaylistItem(ListItem):
    """A single playlist entry in the list."""

    def __init__(self, name: str, is_current: bool = False) -> None:
        super().__init__()
        self.playlist_name = name
        self.is_current = is_current

    def compose(self) -> ComposeResult:
        if self.is_current:
            yield Label(f"♫ {self.playlist_name}", classes="playlist-playing")
        else:
            yield Label(f"  {self.playlist_name}")


# ── Main App ─────────────────────────────────────────────────────────


class MusicTUI(App):
    """Apple Music TUI — control Apple Music from your terminal."""

    CSS = """
    Screen {
        background: $surface;
    }

    #main-layout {
        height: 1fr;
    }

    #left-panel {
        width: 1fr;
        min-width: 30;
        padding: 0 1;
    }

    #right-panel {
        width: 35;
        min-width: 25;
        padding: 1 2;
        border-left: solid $primary-background-darken-2;
    }

    #now-playing-box {
        height: auto;
        max-height: 6;
        padding: 1 2;
        margin: 0 0 1 0;
        border: round $accent;
    }

    #progress-bar {
        height: 1;
        padding: 0 1;
        margin: 0 0 1 0;
    }

    #controls {
        height: 1;
        padding: 0 2;
        margin: 0 0 1 0;
    }

    #playlist-search {
        display: none;
        margin: 0 0 1 0;
        dock: top;
    }

    #playlist-search.visible {
        display: block;
    }

    #playlist-list {
        height: 1fr;
        border: round $primary-background-darken-1;
    }

    #playlist-list > .list-item {
        padding: 0 1;
    }

    #playlist-list:focus > .list-item.--highlight {
        background: $accent;
        color: $text;
    }

    .playlist-playing {
        color: $success;
    }

    #up-next {
        height: auto;
        margin: 0 0 1 0;
    }

    #keyhints {
        height: auto;
        margin-top: 1;
        padding: 0;
    }

    #status-bar {
        dock: bottom;
        height: 1;
        background: $primary-background-darken-1;
        padding: 0 2;
    }

    Header {
        dock: top;
    }
    """

    TITLE = "♫ Apple Music"
    SUB_TITLE = "Terminal Controller"

    BINDINGS = [
        Binding("space", "play_pause", "Play/Pause", priority=True),
        Binding("n", "next_track", "Next"),
        Binding("p", "prev_track", "Prev"),
        Binding("s", "stop_track", "Stop"),
        Binding("x", "toggle_shuffle", "Shuffle"),
        Binding("v", "toggle_repeat", "Repeat"),
        Binding("equal,plus", "volume_up", "+Vol", key_display="+"),
        Binding("minus", "volume_down", "-Vol", key_display="-"),
        Binding("m", "toggle_mute", "Mute"),
        Binding("right", "seek_forward", ">>", show=False),
        Binding("left", "seek_back", "<<", show=False),
        Binding("slash", "search", "Search", priority=True),
        Binding("r", "refresh", "Refresh", show=False),
        Binding("q", "quit", "Quit"),
    ]

    def __init__(self):
        super().__init__()
        self.music = MusicState()
        self._search_visible = False
        self._all_playlists: list[str] = []
        self._poll_timer: Optional[Timer] = None
        self._tick_timer: Optional[Timer] = None

    def compose(self) -> ComposeResult:
        yield Header()
        with Horizontal(id="main-layout"):
            with Vertical(id="left-panel"):
                yield NowPlaying(id="now-playing-box")
                yield TrackProgress(id="progress-bar")
                yield PlayerControls(id="controls")
                yield Input(placeholder="Search playlists...", id="playlist-search")
                yield ListView(id="playlist-list")
            with Vertical(id="right-panel"):
                yield UpNextPanel(id="up-next")
                yield Static(
                    "[bold dim]SHORTCUTS[/]\n"
                    "  [bold cyan]Space[/]  [dim]play/pause[/]\n"
                    "  [bold cyan]n / p[/]  [dim]next / prev[/]\n"
                    "  [bold cyan]+ / -[/]  [dim]volume[/]\n"
                    "  [bold cyan]← / →[/]  [dim]seek[/]\n"
                    "  [bold cyan]x[/]      [dim]shuffle[/]\n"
                    "  [bold cyan]v[/]      [dim]repeat[/]\n"
                    "  [bold cyan]/ [/]     [dim]search[/]\n"
                    "  [bold cyan]Enter[/]  [dim]play playlist[/]\n"
                    "  [bold cyan]m[/]      [dim]mute[/]\n"
                    "  [bold cyan]q[/]      [dim]quit[/]",
                    id="keyhints",
                )
        yield Static("", id="status-bar")
        yield Footer()

    def on_mount(self) -> None:
        self._set_status("Loading...")
        self.initial_load()
        self._poll_timer = self.set_interval(POLL_INTERVAL, self.poll_state)
        self._tick_timer = self.set_interval(0.5, self.tick_progress)

    @work(thread=True)
    def initial_load(self) -> None:
        playlists = fetch_playlists()
        track = fetch_now_playing()
        vol = fetch_volume()
        shuf = fetch_shuffle()
        rpt = fetch_repeat()
        cp = fetch_current_playlist()
        un, ua = fetch_up_next_ui()
        if not un:
            un, ua = fetch_up_next_playlist()

        self.music.playlists = playlists
        self.music.track = track
        self.music.volume = vol
        self.music.shuffle = shuf
        self.music.repeat_mode = rpt
        self.music.current_playlist = cp
        self.music.up_next_name = un
        self.music.up_next_artist = ua
        self.music.last_position_time = time.time()
        self._all_playlists = list(playlists)

        self.call_from_thread(self._update_all_widgets)
        self.call_from_thread(self._rebuild_playlist_list, playlists, cp)
        self.call_from_thread(self._set_status, f"Loaded {len(playlists)} playlists")

    @work(thread=True)
    def poll_state(self) -> None:
        track = fetch_now_playing()
        vol = fetch_volume()
        shuf = fetch_shuffle()
        rpt = fetch_repeat()
        cp = fetch_current_playlist()
        un, ua = fetch_up_next_ui()
        if not un:
            un, ua = fetch_up_next_playlist()

        self.music.track = track
        self.music.volume = vol
        self.music.shuffle = shuf
        self.music.repeat_mode = rpt
        self.music.current_playlist = cp
        self.music.up_next_name = un
        self.music.up_next_artist = ua
        self.music.last_position_time = time.time()

        self.call_from_thread(self._update_all_widgets)

    def tick_progress(self) -> None:
        t = self.music.track
        if t.state == "PLAYING" and t.duration > 0:
            now = time.time()
            if self.music.last_position_time:
                delta = now - self.music.last_position_time
                t.position = min(t.duration, t.position + delta)
            self.music.last_position_time = now
            try:
                pb = self.query_one("#progress-bar", TrackProgress)
                pb.position = t.position
                pb.duration = t.duration
            except NoMatches:
                pass

    def _update_all_widgets(self) -> None:
        t = self.music.track
        try:
            np = self.query_one("#now-playing-box", NowPlaying)
            np.track = t
        except NoMatches:
            pass

        try:
            pb = self.query_one("#progress-bar", TrackProgress)
            pb.position = t.position
            pb.duration = t.duration
        except NoMatches:
            pass

        try:
            ctrl = self.query_one("#controls", PlayerControls)
            ctrl.shuffle = self.music.shuffle
            ctrl.repeat_mode = self.music.repeat_mode
            ctrl.volume = self.music.volume
        except NoMatches:
            pass

        try:
            un = self.query_one("#up-next", UpNextPanel)
            un.up_next_name = self.music.up_next_name
            un.up_next_artist = self.music.up_next_artist
            un.current_playlist = self.music.current_playlist
        except NoMatches:
            pass

    def _rebuild_playlist_list(self, playlists: list[str], current: str = "") -> None:
        try:
            lv = self.query_one("#playlist-list", ListView)
        except NoMatches:
            return
        lv.clear()
        for name in playlists:
            lv.append(PlaylistItem(name, is_current=(name == current and bool(current))))

    def _set_status(self, msg: str) -> None:
        try:
            sb = self.query_one("#status-bar", Static)
            t = self.music.track
            icon = {"PLAYING": "▶", "PAUSED": "⏸"}.get(t.state, "·")
            sb.update(f" {icon}  {msg}")
        except NoMatches:
            pass

    # ── Actions ───────────────────────────────────────────────────────

    @work(thread=True)
    def action_play_pause(self) -> None:
        cmd_play_pause()
        self.call_from_thread(self._set_status, "Toggled play/pause")

    @work(thread=True)
    def action_next_track(self) -> None:
        cmd_next()
        self.call_from_thread(self._set_status, "Next track")

    @work(thread=True)
    def action_prev_track(self) -> None:
        cmd_prev()
        self.call_from_thread(self._set_status, "Previous track")

    @work(thread=True)
    def action_stop_track(self) -> None:
        cmd_stop()
        self.call_from_thread(self._set_status, "Stopped")

    @work(thread=True)
    def action_toggle_shuffle(self) -> None:
        cmd_toggle_shuffle()
        self.music.shuffle = fetch_shuffle()
        self.call_from_thread(self._update_all_widgets)
        state = "on" if self.music.shuffle else "off"
        self.call_from_thread(self._set_status, f"Shuffle {state}")

    @work(thread=True)
    def action_toggle_repeat(self) -> None:
        cycle = {"off": "all", "all": "one", "one": "off"}
        current = self.music.repeat_mode or "off"
        next_mode = cycle.get(current, "off")
        cmd_set_repeat(next_mode)
        self.music.repeat_mode = next_mode
        self.call_from_thread(self._update_all_widgets)
        self.call_from_thread(self._set_status, f"Repeat: {next_mode}")

    @work(thread=True)
    def action_volume_up(self) -> None:
        if self.music.volume < 0:
            self.music.volume = fetch_volume()
        new = min(100, self.music.volume + 5)
        cmd_set_volume(new)
        self.music.volume = new
        self.call_from_thread(self._update_all_widgets)
        self.call_from_thread(self._set_status, f"Volume: {new}%")

    @work(thread=True)
    def action_volume_down(self) -> None:
        if self.music.volume < 0:
            self.music.volume = fetch_volume()
        new = max(0, self.music.volume - 5)
        cmd_set_volume(new)
        self.music.volume = new
        self.call_from_thread(self._update_all_widgets)
        self.call_from_thread(self._set_status, f"Volume: {new}%")

    @work(thread=True)
    def action_toggle_mute(self) -> None:
        if self.music.volume < 0:
            self.music.volume = fetch_volume()
        if self.music.volume > 0:
            self.music._pre_mute_vol = self.music.volume
            new = 0
        else:
            new = self.music._pre_mute_vol
        cmd_set_volume(new)
        self.music.volume = new
        self.call_from_thread(self._update_all_widgets)
        msg = "Muted" if new == 0 else f"Unmuted ({new}%)"
        self.call_from_thread(self._set_status, msg)

    @work(thread=True)
    def action_seek_forward(self) -> None:
        t = self.music.track
        if t.state not in ("PLAYING", "PAUSED"):
            return
        new_pos = min(t.duration, t.position + 10.0)
        cmd_seek(new_pos)
        t.position = new_pos
        self.music.last_position_time = time.time()
        self.call_from_thread(self._update_all_widgets)
        self.call_from_thread(self._set_status, "Seek forward 10s")

    @work(thread=True)
    def action_seek_back(self) -> None:
        t = self.music.track
        if t.state not in ("PLAYING", "PAUSED"):
            return
        new_pos = max(0.0, t.position - 10.0)
        cmd_seek(new_pos)
        t.position = new_pos
        self.music.last_position_time = time.time()
        self.call_from_thread(self._update_all_widgets)
        self.call_from_thread(self._set_status, "Seek back 10s")

    def action_search(self) -> None:
        search_input = self.query_one("#playlist-search", Input)
        if self._search_visible:
            self._search_visible = False
            search_input.remove_class("visible")
            search_input.value = ""
            self._rebuild_playlist_list(self._all_playlists, self.music.current_playlist)
            self.query_one("#playlist-list", ListView).focus()
        else:
            self._search_visible = True
            search_input.add_class("visible")
            search_input.focus()

    @work(thread=True)
    def action_refresh(self) -> None:
        playlists = fetch_playlists()
        self.music.playlists = playlists
        self._all_playlists = list(playlists)
        cp = self.music.current_playlist
        self.call_from_thread(self._rebuild_playlist_list, playlists, cp)
        self.call_from_thread(self._set_status, f"Refreshed — {len(playlists)} playlists")

    # ── Events ────────────────────────────────────────────────────────

    @on(Input.Changed, "#playlist-search")
    def on_search_changed(self, event: Input.Changed) -> None:
        query = event.value.lower()
        if not query:
            filtered = self._all_playlists
        else:
            filtered = [p for p in self._all_playlists if query in p.lower()]
        self._rebuild_playlist_list(filtered, self.music.current_playlist)

    @on(Input.Submitted, "#playlist-search")
    def on_search_submitted(self, event: Input.Submitted) -> None:
        query = event.value.lower()
        if query:
            filtered = [p for p in self._all_playlists if query in p.lower()]
        else:
            filtered = self._all_playlists
        if filtered:
            self._play_playlist_by_name(filtered[0])
        self._search_visible = False
        search_input = self.query_one("#playlist-search", Input)
        search_input.remove_class("visible")
        search_input.value = ""
        self._rebuild_playlist_list(self._all_playlists, self.music.current_playlist)
        self.query_one("#playlist-list", ListView).focus()

    @on(ListView.Selected, "#playlist-list")
    def on_playlist_selected(self, event: ListView.Selected) -> None:
        item = event.item
        if isinstance(item, PlaylistItem):
            self._play_playlist_by_name(item.playlist_name)

    @work(thread=True)
    def _play_playlist_by_name(self, name: str) -> None:
        cmd_play_playlist(name)
        self.call_from_thread(self._set_status, f"Playing: {name}")

    def on_key(self, event) -> None:
        # Let Escape close search if open
        if event.key == "escape" and self._search_visible:
            self._search_visible = False
            search_input = self.query_one("#playlist-search", Input)
            search_input.remove_class("visible")
            search_input.value = ""
            self._rebuild_playlist_list(self._all_playlists, self.music.current_playlist)
            self.query_one("#playlist-list", ListView).focus()
            event.prevent_default()


# ── Entry point ──────────────────────────────────────────────────────

def main():
    app = MusicTUI()
    app.run()


if __name__ == "__main__":
    main()
