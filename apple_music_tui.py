#!/usr/bin/env python3
import curses
import locale
import os
import subprocess
import sys
import threading
import time
from dataclasses import dataclass, field
from typing import List, Optional, Tuple

APP_NAME = "Music"
POLL_INTERVAL = 2.0
APPLESCRIPT_TIMEOUT = 5.0
STATUS_CLEAR_SECONDS = 5.0


def init_locale() -> None:
    for loc in ("en_US.UTF-8", "C.UTF-8", "UTF-8", ""):
        try:
            locale.setlocale(locale.LC_ALL, loc)
            return
        except locale.Error:
            continue


init_locale()

TERM_NAME = os.environ.get("TERM", "").lower()
USE_ASCII = os.environ.get("MUSICTUI_ASCII", "") == "1" or "ghostty" in TERM_NAME


@dataclass
class TrackInfo:
    name: str = ""
    artist: str = ""
    album: str = ""
    state: str = "STOPPED"
    duration: float = 0.0
    position: float = 0.0


@dataclass
class UpNextInfo:
    name: str = ""
    artist: str = ""
    album: str = ""
    status: str = "UNKNOWN"


@dataclass
class AppState:
    playlists: List[str] = field(default_factory=list)
    selected_index: int = 0
    now_playing: TrackInfo = field(default_factory=TrackInfo)
    up_next: UpNextInfo = field(default_factory=UpNextInfo)
    up_next_source: str = "playlist"
    status: str = ""
    status_set_time: float = 0.0
    last_poll: float = 0.0
    last_position_time: float = 0.0
    shuffle_enabled: Optional[bool] = None
    volume: int = -1
    repeat_mode: Optional[str] = None
    current_playlist_name: str = ""
    search_query: str = ""
    search_active: bool = False
    show_help: bool = False
    controls: dict = field(default_factory=dict)
    playlist_box_info: Tuple[int, int, int, int] = (0, 0, 0, 0)
    lock: threading.Lock = field(default_factory=threading.Lock)
    stop_event: threading.Event = field(default_factory=threading.Event)
    playlists_loaded: bool = False


# ── AppleScript helpers ──────────────────────────────────────────────


def set_status(state, msg: str) -> None:
    state.status = msg
    state.status_set_time = time.time()


def run_applescript(script: str, timeout: float = APPLESCRIPT_TIMEOUT) -> Tuple[str, str, int]:
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


def dump_music_ui(state: AppState) -> None:
    script = f'''
    on walk_element(el, depth)
        set pad to ""
        repeat depth times
            set pad to pad & "  "
        end repeat
        set line to pad
        try
            set line to line & (role of el as text)
        on error
            set line to line & "UNKNOWN_ROLE"
        end try
        try
            set sr to subrole of el as text
            set line to line & " / " & sr
        end try
        try
            set nm to name of el as text
            if nm is not "" then set line to line & " | " & nm
        end try
        set output to line & "\\n"
        if depth < 6 then
            try
                set kids to UI elements of el
                repeat with child in kids
                    set output to output & my walk_element(child, depth + 1)
                end repeat
            end try
        end if
        return output
    end walk_element

    tell application "System Events"
        if not (exists process "{APP_NAME}") then
            return "NO_PROCESS"
        end if
        tell process "{APP_NAME}"
            if not (exists window 1) then
                return "NO_WINDOW"
            end if
            try
                return my walk_element(window 1, 0)
            on error errMsg number errNum
                return "ERR:" & errNum & ":" & errMsg
            end try
        end tell
    end tell
    '''
    out, err, code = run_applescript(script)
    if err or code != 0 or out.startswith("ERR:"):
        set_status(state, "Failed to dump UI. Ensure Accessibility is enabled.")
        return
    path = "/tmp/musictui_upnext.txt"
    try:
        with open(path, "w", encoding="utf-8") as handle:
            handle.write(out)
        set_status(state, f"UI dump saved: {path}")
    except OSError:
        set_status(state, "Failed to write UI dump.")


def applescript_escape(value: str) -> str:
    escaped = value.replace("\\", "\\\\").replace('"', '\\"')
    return escaped.replace("\n", " ").replace("\r", " ")


def format_error(err: str) -> str:
    if not err:
        return ""
    lowered = err.lower()
    if "not authorized" in lowered or "not authorised" in lowered or "not permitted" in lowered:
        return "Permission denied. Enable Automation for your terminal in System Settings > Privacy & Security > Automation."
    return err


def parse_applescript_number(raw: str) -> float:
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


# ── Fetchers ─────────────────────────────────────────────────────────


def fetch_now_playing(state: AppState) -> None:
    script = f'''
    tell application "{APP_NAME}"
        if it is running then
            if player state is stopped then
                return "STOPPED"
            end if
            set t to current track
            set n to name of t
            set a to artist of t
            set al to album of t
            set s to player state as string
            set d to duration of t
            set p to player position
            return n & "\\n" & a & "\\n" & al & "\\n" & s & "\\n" & d & "\\n" & p
        end if
    end tell
    return "NOT_RUNNING"
    '''
    out, err, code = run_applescript(script)
    err_msg = format_error(err)
    if err_msg:
        set_status(state, err_msg)
        return
    if code != 0:
        set_status(state, "AppleScript failed.")
        return
    if out in ("STOPPED", "NOT_RUNNING", ""):
        state.now_playing = TrackInfo(state=out or "UNKNOWN")
        return
    parts = out.split("\n")
    if len(parts) >= 6:
        duration = parse_applescript_number(parts[4])
        position = parse_applescript_number(parts[5])
        state.now_playing = TrackInfo(
            name=parts[0],
            artist=parts[1],
            album=parts[2],
            state=parts[3].upper(),
            duration=duration,
            position=position,
        )
        state.last_position_time = time.time()
    elif len(parts) >= 4:
        state.now_playing = TrackInfo(
            name=parts[0],
            artist=parts[1],
            album=parts[2],
            state=parts[3].upper(),
        )
        state.last_position_time = time.time()


def fetch_up_next(state: AppState) -> None:
    ui_info = fetch_up_next_ui()
    if ui_info:
        state.up_next = ui_info
        state.up_next_source = "ui"
        return
    state.up_next_source = "playlist"
    fetch_up_next_playlist(state)


def fetch_up_next_ui() -> Optional[UpNextInfo]:
    script = f'''
    tell application "System Events"
        if not (exists process "{APP_NAME}") then
            return "NO_PROCESS"
        end if
        tell process "{APP_NAME}"
            if not (exists window 1) then
                return "NO_WINDOW"
            end if
            try
                set theTable to first table of scroll area 1 of window 1
                set row1 to first row of theTable
                set texts to value of static text of row1
                if (count of texts) >= 2 then
                    return item 1 of texts & "\\n" & item 2 of texts
                else if (count of texts) = 1 then
                    return item 1 of texts
                else
                    return "NO_TEXT"
                end if
            on error errMsg number errNum
                return "ERR:" & errNum & ":" & errMsg
            end try
        end tell
    end tell
    '''
    out, err, code = run_applescript(script)
    if err or code != 0:
        return None
    if out.startswith("ERR:") or out in ("NO_PROCESS", "NO_WINDOW", "NO_TEXT", ""):
        return None
    parts = out.split("\n")
    name = parts[0] if parts else ""
    artist = parts[1] if len(parts) > 1 else ""
    return UpNextInfo(name=name, artist=artist, album="", status="OK")


def fetch_up_next_playlist(state: AppState) -> None:
    script = f'''
    tell application "{APP_NAME}"
        if it is running then
            if player state is stopped then
                return "STOPPED"
            end if
            try
                set cp to current playlist
                set ct to current track
                set pid to persistent ID of ct
                set tracksList to tracks of cp
                repeat with i from 1 to count of tracksList
                    if persistent ID of item i of tracksList is pid then
                        if i < count of tracksList then
                            set nt to item (i + 1) of tracksList
                            return name of nt & "\\n" & artist of nt & "\\n" & album of nt
                        else
                            return "END"
                        end if
                    end if
                end repeat
                return "UNKNOWN"
            on error
                return "UNKNOWN"
            end try
        end if
    end tell
    return "NOT_RUNNING"
    '''
    out, err, code = run_applescript(script)
    err_msg = format_error(err)
    if err_msg or code != 0:
        state.up_next = UpNextInfo(status="ERROR")
        return
    if out in ("STOPPED", "NOT_RUNNING", "UNKNOWN", "END", ""):
        state.up_next = UpNextInfo(status=out or "UNKNOWN")
        return
    parts = out.split("\n")
    if len(parts) >= 3:
        state.up_next = UpNextInfo(
            name=parts[0],
            artist=parts[1],
            album=parts[2],
            status="OK",
        )


def fetch_playlists(state: AppState) -> None:
    script = f'''
    set AppleScript's text item delimiters to "\\n"
    tell application "{APP_NAME}"
        if it is running then
            set plist to name of playlists
            return plist as text
        end if
    end tell
    return "NOT_RUNNING"
    '''
    out, err, code = run_applescript(script)
    err_msg = format_error(err)
    if err_msg:
        set_status(state, err_msg)
        return
    if code != 0:
        set_status(state, "AppleScript failed.")
        return
    if out in ("NOT_RUNNING", ""):
        state.playlists = []
        state.selected_index = 0
        set_status(state, "Music app is not running.")
        return
    playlists = [p.strip() for p in out.split("\n") if p.strip()]
    state.playlists = playlists
    if state.selected_index >= len(playlists):
        state.selected_index = max(0, len(playlists) - 1)
    set_status(state, f"Loaded {len(playlists)} playlists.")


# ── Actions ──────────────────────────────────────────────────────────


def play_pause(state: AppState) -> None:
    _, err, code = run_applescript(f'tell application "{APP_NAME}" to playpause')
    err_msg = format_error(err)
    if err_msg:
        set_status(state, err_msg)
    elif code != 0:
        set_status(state, "AppleScript failed.")
    else:
        set_status(state, "Toggled play/pause.")


def next_track(state: AppState) -> None:
    _, err, code = run_applescript(f'tell application "{APP_NAME}" to next track')
    err_msg = format_error(err)
    if err_msg:
        set_status(state, err_msg)
    elif code != 0:
        set_status(state, "AppleScript failed.")
    else:
        set_status(state, "Next track.")


def previous_track(state: AppState) -> None:
    _, err, code = run_applescript(f'tell application "{APP_NAME}" to previous track')
    err_msg = format_error(err)
    if err_msg:
        set_status(state, err_msg)
    elif code != 0:
        set_status(state, "AppleScript failed.")
    else:
        set_status(state, "Previous track.")


def play_selected_playlist(state: AppState) -> None:
    if not state.playlists:
        set_status(state, "No playlists found.")
        return
    name = applescript_escape(state.playlists[state.selected_index])
    script = f'''
    tell application "{APP_NAME}"
        play playlist "{name}"
    end tell
    '''
    _, err, code = run_applescript(script)
    err_msg = format_error(err)
    if err_msg:
        set_status(state, err_msg)
    elif code != 0:
        set_status(state, "AppleScript failed.")
    else:
        set_status(state, f"Playing playlist: {name}")


def toggle_shuffle(state: AppState) -> None:
    script = f'''
    tell application "{APP_NAME}"
        if it is running then
            try
                set thePlaylist to current playlist
                set shuffle enabled of thePlaylist to not shuffle enabled of thePlaylist
                return shuffle enabled of thePlaylist as string
            on error errMsg number errNum
                try
                    set shuffle enabled to not shuffle enabled
                    return shuffle enabled as string
                on error errMsg2 number errNum2
                    return "ERR:" & errNum2 & ":" & errMsg2
                end try
            end try
        end if
    end tell
    return "NOT_RUNNING"
    '''
    out, err, code = run_applescript(script)
    err_msg = format_error(err)
    if err_msg:
        set_status(state, err_msg)
    elif code != 0:
        set_status(state, "AppleScript failed.")
    elif out.startswith("ERR:"):
        set_status(state, "Shuffle failed.")
    elif out == "true":
        set_status(state, "Shuffle on.")
    elif out == "false":
        set_status(state, "Shuffle off.")
    else:
        set_status(state, "Toggled shuffle.")
    fetch_shuffle_state(state)


def fetch_shuffle_state(state: AppState) -> None:
    script = f'''
    tell application "{APP_NAME}"
        if it is running then
            try
                set thePlaylist to current playlist
                return shuffle enabled of thePlaylist as string
            on error
                try
                    return shuffle enabled as string
                on error
                    return "UNKNOWN"
                end try
            end try
        end if
    end tell
    return "UNKNOWN"
    '''
    out, err, code = run_applescript(script)
    err_msg = format_error(err)
    if err_msg or code != 0:
        state.shuffle_enabled = None
        return
    if out == "true":
        state.shuffle_enabled = True
    elif out == "false":
        state.shuffle_enabled = False
    else:
        state.shuffle_enabled = None


def fetch_volume(state: AppState) -> None:
    script = f'''
    tell application "{APP_NAME}"
        if it is running then
            return sound volume as string
        end if
    end tell
    return "-1"
    '''
    out, err, code = run_applescript(script)
    if err or code != 0:
        return
    try:
        state.volume = int(parse_applescript_number(out))
    except (ValueError, TypeError):
        pass


def set_volume(state: AppState, delta: int) -> None:
    if state.volume < 0:
        fetch_volume(state)
    new_vol = max(0, min(100, state.volume + delta))
    script = f'tell application "{APP_NAME}" to set sound volume to {new_vol}'
    _, err, code = run_applescript(script)
    if err or code != 0:
        set_status(state, "Volume change failed.")
    else:
        state.volume = new_vol
        set_status(state, f"Volume: {new_vol}%")


def toggle_mute(state: AppState) -> None:
    if state.volume < 0:
        fetch_volume(state)
    if state.volume > 0:
        state._pre_mute_volume = state.volume
        new_vol = 0
    else:
        new_vol = getattr(state, "_pre_mute_volume", 50)
    script = f'tell application "{APP_NAME}" to set sound volume to {new_vol}'
    _, err, code = run_applescript(script)
    if err or code != 0:
        set_status(state, "Mute toggle failed.")
    else:
        state.volume = new_vol
        set_status(state, "Muted." if new_vol == 0 else f"Unmuted ({new_vol}%).")


def seek_track(state: AppState, delta: float) -> None:
    info = state.now_playing
    if info.state not in ("PLAYING", "PAUSED"):
        set_status(state, "Nothing to seek.")
        return
    new_pos = max(0.0, min(info.duration, info.position + delta))
    script = f'tell application "{APP_NAME}" to set player position to {new_pos}'
    _, err, code = run_applescript(script)
    if err or code != 0:
        set_status(state, "Seek failed.")
    else:
        state.now_playing.position = new_pos
        state.last_position_time = time.time()
        direction = "forward" if delta > 0 else "back"
        set_status(state, f"Seek {direction} {abs(int(delta))}s.")


def fetch_repeat_mode(state: AppState) -> None:
    script = f'''
    tell application "{APP_NAME}"
        if it is running then
            try
                return song repeat as string
            on error
                return "UNKNOWN"
            end try
        end if
    end tell
    return "UNKNOWN"
    '''
    out, err, code = run_applescript(script)
    if err or code != 0:
        state.repeat_mode = None
        return
    out_lower = out.strip().lower()
    if out_lower in ("off", "none"):
        state.repeat_mode = "off"
    elif out_lower in ("one",):
        state.repeat_mode = "one"
    elif out_lower in ("all",):
        state.repeat_mode = "all"
    else:
        state.repeat_mode = None


def toggle_repeat(state: AppState) -> None:
    cycle = {"off": "all", "all": "one", "one": "off"}
    current = state.repeat_mode or "off"
    next_mode = cycle.get(current, "off")
    as_val = {"off": "off", "one": "one", "all": "all"}[next_mode]
    script = f'tell application "{APP_NAME}" to set song repeat to {as_val}'
    _, err, code = run_applescript(script)
    if err or code != 0:
        set_status(state, "Repeat toggle failed.")
    else:
        state.repeat_mode = next_mode
        set_status(state, f"Repeat: {next_mode}.")


def fetch_current_playlist_name(state: AppState) -> None:
    script = f'''
    tell application "{APP_NAME}"
        if it is running then
            if player state is not stopped then
                try
                    return name of current playlist
                on error
                    return ""
                end try
            end if
        end if
    end tell
    return ""
    '''
    out, err, code = run_applescript(script)
    if err or code != 0:
        state.current_playlist_name = ""
        return
    state.current_playlist_name = out.strip()


def play_track(state: AppState) -> None:
    _, err, code = run_applescript(f'tell application "{APP_NAME}" to play')
    err_msg = format_error(err)
    if err_msg:
        set_status(state, err_msg)
    elif code != 0:
        set_status(state, "AppleScript failed.")
    else:
        set_status(state, "Play.")


def pause_track(state: AppState) -> None:
    _, err, code = run_applescript(f'tell application "{APP_NAME}" to pause')
    err_msg = format_error(err)
    if err_msg:
        set_status(state, err_msg)
    elif code != 0:
        set_status(state, "AppleScript failed.")
    else:
        set_status(state, "Pause.")


def stop_track(state: AppState) -> None:
    _, err, code = run_applescript(f'tell application "{APP_NAME}" to stop')
    err_msg = format_error(err)
    if err_msg:
        set_status(state, err_msg)
    elif code != 0:
        set_status(state, "AppleScript failed.")
    else:
        set_status(state, "Stop.")


def background_poll(state: AppState) -> None:
    fetch_playlists(state)
    with state.lock:
        state.playlists_loaded = True
    fetch_now_playing(state)
    fetch_up_next(state)
    fetch_shuffle_state(state)
    fetch_volume(state)
    fetch_repeat_mode(state)
    fetch_current_playlist_name(state)

    poll_count = 0
    playlist_refresh_interval = 15

    while not state.stop_event.is_set():
        state.stop_event.wait(POLL_INTERVAL)
        if state.stop_event.is_set():
            break
        poll_count += 1
        fetch_now_playing(state)
        fetch_up_next(state)
        fetch_shuffle_state(state)
        fetch_volume(state)
        fetch_repeat_mode(state)
        fetch_current_playlist_name(state)
        if poll_count >= playlist_refresh_interval:
            fetch_playlists(state)
            poll_count = 0


# ── Format helpers ───────────────────────────────────────────────────


def format_time(seconds: float) -> str:
    if seconds <= 0:
        return "--:--"
    total = int(seconds)
    minutes = total // 60
    secs = total % 60
    return f"{minutes}:{secs:02d}"


# ── Color system ─────────────────────────────────────────────────────

C_HEADER = 1
C_DIM = 2
C_SELECTED_PLAY = 3
C_SELECTED_PAUSE = 4
C_SELECTED_STOP = 5
C_ACCENT_PLAY = 6
C_ACCENT_PAUSE = 7
C_ACCENT_STOP = 8
C_STATUS = 9
C_MUTED = 10
C_BRIGHT = 11
C_PROGRESS = 12
C_ART = 13

# Equalizer animation frames (6 frames, cycle at ~4Hz)
EQ_FRAMES = [
    "▁▃▅▇▅▃",
    "▃▅▇▅▃▁",
    "▅▇▅▃▁▃",
    "▇▅▃▁▃▅",
    "▅▃▁▃▅▇",
    "▃▁▃▅▇▅",
]

# Box-drawing characters (panel borders)
if USE_ASCII:
    BOX_TL, BOX_TR, BOX_BL, BOX_BR = "+", "+", "+", "+"
    BOX_H, BOX_V = "-", "|"
    PROG_FILLED, PROG_HEAD, PROG_EMPTY = "=", "O", "-"
else:
    BOX_TL, BOX_TR, BOX_BL, BOX_BR = "\u256d", "\u256e", "\u2570", "\u256f"
    BOX_H, BOX_V = "\u2500", "\u2502"
    PROG_FILLED, PROG_HEAD, PROG_EMPTY = "\u2501", "\u25cf", "\u254c"


def init_colors() -> None:
    curses.start_color()
    curses.use_default_colors()
    curses.init_pair(C_HEADER, curses.COLOR_BLACK, curses.COLOR_BLUE)
    curses.init_pair(C_DIM, curses.COLOR_WHITE, -1)
    curses.init_pair(C_SELECTED_PLAY, curses.COLOR_WHITE, curses.COLOR_BLUE)
    curses.init_pair(C_SELECTED_PAUSE, curses.COLOR_WHITE, curses.COLOR_MAGENTA)
    curses.init_pair(C_SELECTED_STOP, curses.COLOR_BLACK, curses.COLOR_WHITE)
    curses.init_pair(C_ACCENT_PLAY, curses.COLOR_BLUE, -1)
    curses.init_pair(C_ACCENT_PAUSE, curses.COLOR_MAGENTA, -1)
    curses.init_pair(C_ACCENT_STOP, curses.COLOR_WHITE, -1)
    curses.init_pair(C_STATUS, curses.COLOR_BLACK, curses.COLOR_WHITE)
    curses.init_pair(C_MUTED, curses.COLOR_WHITE, -1)
    curses.init_pair(C_BRIGHT, curses.COLOR_WHITE, -1)
    curses.init_pair(C_PROGRESS, curses.COLOR_CYAN, -1)
    curses.init_pair(C_ART, curses.COLOR_BLUE, -1)


def accent_pair(state: AppState) -> int:
    ps = state.now_playing.state.upper() if state.now_playing.state else ""
    if ps == "PLAYING":
        return C_ACCENT_PLAY
    if ps == "PAUSED":
        return C_ACCENT_PAUSE
    return C_ACCENT_STOP


def selected_pair(state: AppState) -> int:
    ps = state.now_playing.state.upper() if state.now_playing.state else ""
    if ps == "PLAYING":
        return C_SELECTED_PLAY
    if ps == "PAUSED":
        return C_SELECTED_PAUSE
    return C_SELECTED_STOP


# ── Safe addstr (avoids curses crash at bottom-right corner) ─────────


def safe_addstr(stdscr, y, x, text, attr=0):
    h, w = stdscr.getmaxyx()
    if y < 0 or y >= h or x >= w:
        return
    max_len = w - x
    if max_len <= 0:
        return
    truncated = text[:max_len]
    # Last cell of terminal: addstr would advance cursor past end
    if y == h - 1 and x + len(truncated) >= w:
        truncated = truncated[: max_len - 1]
    if truncated:
        try:
            stdscr.addstr(y, x, truncated, attr)
        except curses.error:
            pass


def draw_panel_border(stdscr, y, x, h, w, title="", attr=0):
    if h < 2 or w < 4:
        return
    dim = curses.color_pair(C_DIM) | curses.A_DIM
    # Top border: ╭─ Title ────╮
    if title:
        top_line = BOX_TL + BOX_H + " " + title + " " + BOX_H * max(0, w - len(title) - 5) + BOX_TR
    else:
        top_line = BOX_TL + BOX_H * (w - 2) + BOX_TR
    safe_addstr(stdscr, y, x, top_line[:w], dim)
    if title:
        safe_addstr(stdscr, y, x + 3, title, attr | curses.A_BOLD)
    # Side borders
    for row in range(1, h - 1):
        safe_addstr(stdscr, y + row, x, BOX_V, dim)
        safe_addstr(stdscr, y + row, x + w - 1, BOX_V, dim)
    # Bottom border: ╰───────────╯
    bot_line = BOX_BL + BOX_H * (w - 2) + BOX_BR
    safe_addstr(stdscr, y + h - 1, x, bot_line[:w], dim)


# ── Drawing ──────────────────────────────────────────────────────────


def draw_header(stdscr, width, state: AppState) -> None:
    if width <= 0:
        return
    ps = state.now_playing.state.upper() if state.now_playing.state else "UNKNOWN"
    bar_attr = curses.color_pair(C_HEADER) | curses.A_BOLD
    safe_addstr(stdscr, 0, 0, " " * (width - 1), bar_attr)

    left = " Apple Music" if USE_ASCII else " ♫ Apple Music"
    safe_addstr(stdscr, 0, 0, left, bar_attr)

    if USE_ASCII:
        if ps == "PLAYING":
            indicator = ">>> playing"
        elif ps == "PAUSED":
            indicator = "|| paused"
        else:
            indicator = ". stopped"
    else:
        if ps == "PLAYING":
            frame_idx = int(time.time() * 4) % len(EQ_FRAMES)
            eq = EQ_FRAMES[frame_idx]
            indicator = f"{eq} playing"
        elif ps == "PAUSED":
            indicator = ("◐" if int(time.time()) % 2 == 0 else "◑") + " paused"
        else:
            indicator = "○ stopped"
    right = f" {indicator} "
    if width > len(right) + len(left) + 1:
        safe_addstr(stdscr, 0, width - len(right) - 1, right, bar_attr)


def draw_now_playing(stdscr, y, x, h, w, state: AppState) -> None:
    if h < 3 or w < 10:
        return
    info = state.now_playing
    acc = curses.color_pair(accent_pair(state))
    acc_bold = acc | curses.A_BOLD
    dim = curses.color_pair(C_DIM) | curses.A_DIM
    bright = curses.color_pair(C_BRIGHT) | curses.A_BOLD

    text_x = x + 1
    tw = w - 2

    if info.state in ("NOT_RUNNING", "STOPPED", "UNKNOWN", ""):
        msg = "Music app not running." if info.state == "NOT_RUNNING" else "Nothing playing."
        safe_addstr(stdscr, y + 1, text_x, msg, dim)
        return

    line = y

    # Row 0: Track title (bold white)
    safe_addstr(stdscr, line, text_x, (info.name or "Untitled")[:tw], bright)
    line += 1

    # Row 1: Artist · Album (dimmed)
    if line < y + h:
        artist = info.artist or "Unknown"
        album = info.album or "Unknown"
        sep = "  -  " if USE_ASCII else "  \u00b7  "
        safe_addstr(stdscr, line, text_x, f"{artist}{sep}{album}"[:tw], dim)
        line += 1

    # Row 2: blank
    line += 1

    # Row 3: ● Playing   ⇆ On   ↻ All   ♪ 72%
    if line < y + h:
        ps = info.state.upper()
        if USE_ASCII:
            sd = {"PLAYING": ">", "PAUSED": "||"}.get(ps, ".")
            si, ri, vi = "~", "R", "#"
        else:
            sd = {"PLAYING": "\u25cf", "PAUSED": "\u23f8"}.get(ps, "\u25cb")
            si, ri, vi = "\u21c6", "\u21bb", "\u266a"
        shuf = "On" if state.shuffle_enabled else "Off"
        if state.shuffle_enabled is None:
            shuf = "-"
        rpt = (state.repeat_mode or "-").capitalize()
        vol_str = f"   {vi} {state.volume}%" if state.volume >= 0 else ""
        chips = f"{sd} {ps.capitalize()}   {si} {shuf}   {ri} {rpt}{vol_str}"
        safe_addstr(stdscr, line, text_x, chips[:tw], acc)
        line += 1

    # Row 4: blank
    line += 1

    # Row 5: full-width progress bar
    prog_y = y + h - 1
    if prog_y >= line:
        time_l = format_time(info.position)
        time_r = format_time(info.duration)
        bar_w = tw - len(time_l) - len(time_r) - 2
        if bar_w >= 8:
            safe_addstr(stdscr, prog_y, text_x, time_l, dim)
            bar_x = text_x + len(time_l) + 1
            if info.duration > 0:
                ratio = max(0.0, min(1.0, info.position / info.duration))
            else:
                ratio = 0.0
            filled = int(ratio * bar_w)
            filled_str = PROG_FILLED * filled
            dot_str = PROG_HEAD if filled < bar_w else ""
            empty_str = PROG_EMPTY * max(0, bar_w - filled - 1)
            safe_addstr(stdscr, prog_y, bar_x, filled_str, acc_bold)
            safe_addstr(stdscr, prog_y, bar_x + filled, dot_str, bright)
            safe_addstr(stdscr, prog_y, bar_x + filled + len(dot_str), empty_str, dim)
            time_r_x = bar_x + bar_w + 1
            safe_addstr(stdscr, prog_y, time_r_x, time_r, dim)
        else:
            safe_addstr(stdscr, prog_y, text_x, f"{time_l} / {time_r}", dim)


def draw_sidebar(stdscr, y, x, h, w, state: AppState) -> None:
    """Right sidebar: Up Next track + playing from + key hints (no headers)."""
    if h < 2 or w < 12:
        return
    dim = curses.color_pair(C_DIM) | curses.A_DIM
    acc = curses.color_pair(accent_pair(state))
    bright = curses.color_pair(C_BRIGHT) | curses.A_BOLD
    cx = x + 2
    tw = w - 3
    line = y

    # Up next track name + artist (no header)
    info = state.up_next
    if info.status == "OK":
        safe_addstr(stdscr, line, cx, (info.name or "Untitled")[:tw], bright)
        line += 1
        if line < y + h:
            safe_addstr(stdscr, line, cx, (info.artist or "Unknown")[:tw], dim)
            line += 1
    elif info.status == "END":
        safe_addstr(stdscr, line, cx, "End of playlist", dim)
        line += 1
    else:
        safe_addstr(stdscr, line, cx, "-", dim)
        line += 1

    line += 1
    if line >= y + h:
        return

    # "from [playlist]" (no header)
    pname = state.current_playlist_name
    if pname:
        safe_addstr(stdscr, line, cx, f"from {pname}"[:tw], dim)
        line += 1

    line += 1
    if line >= y + h:
        return

    # Key hints (no "Keys" header)
    hints = [
        ("space", "play/pause"),
        ("n/p", "next/prev"),
        ("+/-", "volume"),
        ("/", "search"),
    ]
    for key, desc in hints:
        if line >= y + h:
            break
        safe_addstr(stdscr, line, cx, f"{key:<6}", acc | curses.A_BOLD)
        safe_addstr(stdscr, line, cx + 6, desc[:tw - 6], dim)
        line += 1


def get_filtered_playlists(state: AppState) -> List[str]:
    if not state.search_active or not state.search_query:
        return state.playlists
    query = state.search_query.lower()
    return [p for p in state.playlists if query in p.lower()]


def draw_playlists(stdscr, y, x, h, w, state: AppState) -> None:
    if h < 1 or w < 10:
        return
    filtered = get_filtered_playlists(state)
    dim = curses.color_pair(C_DIM) | curses.A_DIM
    acc = curses.color_pair(accent_pair(state))
    sel_cp = curses.color_pair(selected_pair(state))
    cx = x + 2
    tw = w - 4  # leave room for scroll bar

    content_y = y
    max_rows = h
    state.playlist_box_info = (y, x, h, w)

    if not state.playlists:
        msg = "Loading..." if not state.playlists_loaded else "No playlists."
        safe_addstr(stdscr, content_y, cx, msg[:tw], dim)
        return
    if not filtered:
        safe_addstr(stdscr, content_y, cx, "No matches.", dim)
        return

    sel_idx = state.selected_index
    if sel_idx >= len(filtered):
        sel_idx = max(0, len(filtered) - 1)

    start = max(0, sel_idx - max_rows + 1)
    visible = filtered[start: start + max_rows]

    for idx, name in enumerate(visible):
        row = content_y + idx
        if row >= y + h:
            break
        abs_idx = start + idx
        is_playing = (name == state.current_playlist_name and state.current_playlist_name)

        if abs_idx == sel_idx:
            indicator = "\u25b8 " if not USE_ASCII else "> "
            row_text = f"{indicator}{name}"
            padded = row_text[:tw].ljust(tw)
            safe_addstr(stdscr, row, cx, padded, sel_cp | curses.A_BOLD)
        elif is_playing:
            icon = "\u266b " if not USE_ASCII else "# "
            safe_addstr(stdscr, row, cx, (icon + name)[:tw], acc)
        else:
            safe_addstr(stdscr, row, cx, ("  " + name)[:tw])

    # Thin scroll indicator bar
    if len(filtered) > max_rows and h > 2:
        track_x = x + w - 1
        track_h = max_rows
        if track_h > 0 and len(filtered) > 0:
            thumb_h = max(1, track_h * max_rows // len(filtered))
            span = max(1, len(filtered) - max_rows)
            thumb_pos = int((track_h - thumb_h) * start / span) if span > 0 else 0
            scroll_char = BOX_V
            for i in range(track_h):
                row = content_y + i
                if row >= y + h:
                    break
                if thumb_pos <= i < thumb_pos + thumb_h:
                    safe_addstr(stdscr, row, track_x, scroll_char, acc | curses.A_DIM)


def draw_status_bar(stdscr, y, width, state: AppState) -> None:
    if y < 0 or width <= 0:
        return
    status = state.status or "Ready"
    ps = state.now_playing.state.upper() if state.now_playing.state else ""
    if USE_ASCII:
        ind = {"PLAYING": ">", "PAUSED": "=", "STOPPED": "."}.get(ps, " ")
    else:
        ind = {"PLAYING": "▶", "PAUSED": "⏸", "STOPPED": "·"}.get(ps, " ")
    line = f" {ind}  {status}"
    bar_attr = curses.color_pair(C_STATUS)
    safe_addstr(stdscr, y, 0, " " * (width - 1), bar_attr)
    safe_addstr(stdscr, y, 0, line, bar_attr)
    # Right side: ? for help
    hint = "? help "
    if width > len(line) + len(hint) + 2:
        safe_addstr(stdscr, y, width - len(hint) - 1, hint, bar_attr)


def draw_help_overlay(stdscr, height, width, state: AppState) -> None:
    acc = curses.color_pair(accent_pair(state))
    dim = curses.color_pair(C_DIM) | curses.A_DIM
    bright = curses.color_pair(C_BRIGHT) | curses.A_BOLD

    sections = [
        ("PLAYBACK", [
            ("space", "Play / Pause"),
            ("n  p", "Next / Previous"),
            ("o  a  s", "Play / Pause / Stop"),
            ("x", "Shuffle"),
            ("v", "Repeat"),
        ]),
        ("AUDIO", [
            ("+ / -", "Volume up / down"),
            ("m", "Mute / Unmute"),
            ("< / >", "Seek back / forward"),
        ]),
        ("NAVIGATION", [
            ("j  k", "Move down / up"),
            ("enter", "Play playlist"),
            ("g  G", "Top / Bottom"),
            ("/", "Search"),
            ("esc", "Cancel search"),
        ]),
        ("OTHER", [
            ("r", "Refresh"),
            ("u", "Dump UI tree"),
            ("?", "Close help"),
            ("q", "Quit"),
        ]),
    ]

    # Calculate size
    total_lines = 0
    for title, keys in sections:
        total_lines += 1 + len(keys) + 1  # title + keys + gap
    total_lines -= 1  # no trailing gap

    box_w = min(48, width - 4)
    box_h = min(total_lines + 4, height - 2)
    sy = max(0, (height - box_h) // 2)
    sx = max(0, (width - box_w) // 2)

    # Clear area
    for row in range(box_h):
        if sy + row < height:
            safe_addstr(stdscr, sy + row, sx, " " * box_w)

    # Top/bottom accent lines
    line_char = "-" if USE_ASCII else "─"
    safe_addstr(stdscr, sy, sx, line_char * box_w, acc | curses.A_DIM)
    if sy + box_h - 1 < height:
        safe_addstr(stdscr, sy + box_h - 1, sx, line_char * box_w, acc | curses.A_DIM)

    # Title
    safe_addstr(stdscr, sy + 1, sx + 2, "Keyboard Shortcuts", bright)

    # Content
    line = sy + 3
    cx = sx + 2
    kw = 12
    for sec_idx, (title, keys) in enumerate(sections):
        if line >= sy + box_h - 1:
            break
        safe_addstr(stdscr, line, cx, title, acc | curses.A_BOLD)
        line += 1
        for key_str, desc in keys:
            if line >= sy + box_h - 1:
                break
            safe_addstr(stdscr, line, cx + 1, f"{key_str:<{kw}}", bright)
            safe_addstr(stdscr, line, cx + 1 + kw, desc[:box_w - kw - 4], dim)
            line += 1
        line += 1  # gap between sections


# ── Layout ───────────────────────────────────────────────────────────


def draw_ui(stdscr, state: AppState) -> None:
    stdscr.erase()
    height, width = stdscr.getmaxyx()

    PAD_LEFT = 2
    PAD_RIGHT = 2
    content_w = width - PAD_LEFT - PAD_RIGHT

    # Row 0: Header bar (full width)
    draw_header(stdscr, width, state)

    # Last row: Status bar
    status_row = height - 1
    draw_status_bar(stdscr, status_row, width, state)

    if height < 10 or content_w < 20:
        stdscr.refresh()
        return

    # Rows 1-2: breathing room (blank)
    # Rows 3-8: Now playing (6 rows)
    np_top = 3
    np_h = 6
    draw_now_playing(stdscr, np_top, PAD_LEFT, np_h, content_w, state)

    # Row 9: blank spacer
    # Rows 10..height-2: bordered panels
    panel_top = np_top + np_h + 1
    panel_bot = height - 2
    panel_h = panel_bot - panel_top + 1

    if panel_h < 4:
        stdscr.refresh()
        return

    # Build panel titles
    filtered = get_filtered_playlists(state)
    if state.search_active:
        cursor = "\u258f" if not USE_ASCII else "|"
        blink = cursor if int(time.time() * 2) % 2 == 0 else " "
        left_title = f"Search: {state.search_query}{blink}"
    else:
        left_title = f"Library {len(filtered)}"
    right_title = "Up Next"

    dim = curses.color_pair(C_DIM) | curses.A_DIM
    acc = curses.color_pair(accent_pair(state))

    if width >= 55:
        # Wide mode: side-by-side bordered panels
        left_w = content_w // 2
        right_w = content_w - left_w
        left_x = PAD_LEFT
        right_x = PAD_LEFT + left_w

        draw_panel_border(stdscr, panel_top, left_x, panel_h, left_w, left_title, acc)
        draw_panel_border(stdscr, panel_top, right_x, panel_h, right_w, right_title, dim)

        # Inner content (inset 1 from border on all sides)
        inner_top = panel_top + 1
        inner_h = panel_h - 2
        if inner_h > 0:
            draw_playlists(stdscr, inner_top, left_x + 1, inner_h, left_w - 2, state)
            draw_sidebar(stdscr, inner_top, right_x + 1, inner_h, right_w - 2, state)
    else:
        # Narrow mode: no borders, stack vertically
        safe_addstr(stdscr, panel_top, PAD_LEFT + 1, left_title, dim)
        pl_top = panel_top + 1
        pl_h = panel_h - 1
        if pl_h > 0:
            draw_playlists(stdscr, pl_top, PAD_LEFT, pl_h, content_w, state)

    # Help overlay
    if state.show_help:
        draw_help_overlay(stdscr, height, width, state)

    stdscr.refresh()


# ── Input handling ───────────────────────────────────────────────────


def handle_search_key(state: AppState, key: int) -> bool:
    if key == 27:  # Esc
        state.search_active = False
        state.search_query = ""
        return True
    if key in (curses.KEY_ENTER, 10, 13):
        state.search_active = False
        filtered = get_filtered_playlists(state)
        if filtered:
            sel = min(state.selected_index, len(filtered) - 1)
            real_name = filtered[sel]
            try:
                state.selected_index = state.playlists.index(real_name)
            except ValueError:
                pass
            threading.Thread(target=play_selected_playlist, args=(state,), daemon=True).start()
        state.search_query = ""
        return True
    if key in (curses.KEY_BACKSPACE, 127, 8):
        if state.search_query:
            state.search_query = state.search_query[:-1]
        return True
    if key == curses.KEY_DOWN:
        filtered = get_filtered_playlists(state)
        if state.selected_index < len(filtered) - 1:
            state.selected_index += 1
        return True
    if key == curses.KEY_UP:
        if state.selected_index > 0:
            state.selected_index -= 1
        return True
    if 32 <= key <= 126:
        state.search_query += chr(key)
        filtered = get_filtered_playlists(state)
        if state.selected_index >= len(filtered):
            state.selected_index = 0
        return True
    return True


def handle_key(stdscr, state: AppState, key: int) -> bool:
    if state.search_active:
        return handle_search_key(state, key)

    if key in (ord("q"), ord("Q")):
        return False
    if key == ord("?"):
        state.show_help = not state.show_help
        return True
    if state.show_help:
        state.show_help = False
        return True

    if key in (curses.KEY_DOWN, ord("j")):
        filtered = get_filtered_playlists(state)
        if state.selected_index < len(filtered) - 1:
            state.selected_index += 1
    elif key in (curses.KEY_UP, ord("k")):
        if state.selected_index > 0:
            state.selected_index -= 1
    elif key == ord("g"):
        state.selected_index = 0
    elif key == ord("G"):
        filtered = get_filtered_playlists(state)
        if filtered:
            state.selected_index = len(filtered) - 1
    elif key in (curses.KEY_PPAGE,):
        _, _, h, _ = state.playlist_box_info
        page = max(1, h - 4)
        state.selected_index = max(0, state.selected_index - page)
    elif key in (curses.KEY_NPAGE,):
        _, _, h, _ = state.playlist_box_info
        page = max(1, h - 4)
        filtered = get_filtered_playlists(state)
        state.selected_index = min(len(filtered) - 1, state.selected_index + page)
    elif key in (curses.KEY_ENTER, 10, 13):
        threading.Thread(target=play_selected_playlist, args=(state,), daemon=True).start()
    elif key == ord(" "):
        threading.Thread(target=play_pause, args=(state,), daemon=True).start()
    elif key in (ord("n"), ord("N")):
        threading.Thread(target=next_track, args=(state,), daemon=True).start()
    elif key in (ord("p"), ord("P")):
        threading.Thread(target=previous_track, args=(state,), daemon=True).start()
    elif key in (ord("o"), ord("O")):
        threading.Thread(target=play_track, args=(state,), daemon=True).start()
    elif key in (ord("a"), ord("A")):
        threading.Thread(target=pause_track, args=(state,), daemon=True).start()
    elif key in (ord("s"), ord("S")):
        threading.Thread(target=stop_track, args=(state,), daemon=True).start()
    elif key in (ord("x"), ord("X")):
        threading.Thread(target=toggle_shuffle, args=(state,), daemon=True).start()
    elif key == ord("v"):
        threading.Thread(target=toggle_repeat, args=(state,), daemon=True).start()
    elif key in (ord("+"), ord("=")):
        threading.Thread(target=set_volume, args=(state, 5), daemon=True).start()
    elif key == ord("-"):
        threading.Thread(target=set_volume, args=(state, -5), daemon=True).start()
    elif key in (ord("m"), ord("M")):
        threading.Thread(target=toggle_mute, args=(state,), daemon=True).start()
    elif key == curses.KEY_RIGHT:
        threading.Thread(target=seek_track, args=(state, 10.0), daemon=True).start()
    elif key == curses.KEY_LEFT:
        threading.Thread(target=seek_track, args=(state, -10.0), daemon=True).start()
    elif key == ord("/"):
        state.search_active = True
        state.search_query = ""
    elif key in (ord("r"), ord("R")):
        set_status(state, "Refreshing...")
        threading.Thread(target=lambda: (fetch_playlists(state), fetch_now_playing(state)), daemon=True).start()
    elif key in (ord("u"), ord("U")):
        threading.Thread(target=dump_music_ui, args=(state,), daemon=True).start()
    elif key == curses.KEY_MOUSE:
        try:
            _, mx, my, _, mouse_state = curses.getmouse()
        except curses.error:
            return True
        if mouse_state & curses.BUTTON1_CLICKED:
            for name, (cy, cx, cw) in state.controls.items():
                if my == cy and cx <= mx < cx + cw:
                    if name == "Prev":
                        threading.Thread(target=previous_track, args=(state,), daemon=True).start()
                    elif name == "Next":
                        threading.Thread(target=next_track, args=(state,), daemon=True).start()
                    elif name == "Play":
                        threading.Thread(target=play_track, args=(state,), daemon=True).start()
                    elif name == "Pause":
                        threading.Thread(target=pause_track, args=(state,), daemon=True).start()
                    elif name == "Stop":
                        threading.Thread(target=stop_track, args=(state,), daemon=True).start()
                    elif name == "Shuffle":
                        threading.Thread(target=toggle_shuffle, args=(state,), daemon=True).start()
                    return True
            py, px, ph, pw = state.playlist_box_info
            if py <= my < py + ph and px < mx < px + pw - 1:
                filtered = get_filtered_playlists(state)
                if filtered:
                    max_rows = ph
                    sel_idx = state.selected_index
                    if sel_idx >= len(filtered):
                        sel_idx = max(0, len(filtered) - 1)
                    start = max(0, sel_idx - max_rows + 1)
                    clicked_idx = start + (my - py)
                    if 0 <= clicked_idx < len(filtered):
                        if state.search_active:
                            real_name = filtered[clicked_idx]
                            try:
                                state.selected_index = state.playlists.index(real_name)
                            except ValueError:
                                state.selected_index = clicked_idx
                        else:
                            state.selected_index = clicked_idx
    return True


# ── Main ─────────────────────────────────────────────────────────────


def main(stdscr) -> None:
    curses.curs_set(0)
    stdscr.nodelay(True)
    stdscr.keypad(True)
    curses.mousemask(curses.ALL_MOUSE_EVENTS)
    init_colors()

    state = AppState()
    set_status(state, "Loading...")

    poll_thread = threading.Thread(target=background_poll, args=(state,), daemon=True)
    poll_thread.start()

    running = True
    try:
        while running:
            now = time.time()
            if state.now_playing.state == "PLAYING" and state.now_playing.duration > 0:
                if state.last_position_time:
                    delta = max(0.0, now - state.last_position_time)
                    state.now_playing.position = min(
                        state.now_playing.duration, state.now_playing.position + delta
                    )
                state.last_position_time = now
            if (state.status and state.status != "Ready"
                    and state.status_set_time > 0
                    and now - state.status_set_time >= STATUS_CLEAR_SECONDS):
                state.status = "Ready"
            draw_ui(stdscr, state)
            key = stdscr.getch()
            if key != -1:
                running = handle_key(stdscr, state, key)
            time.sleep(0.03)
    finally:
        state.stop_event.set()
        poll_thread.join(timeout=1.0)


def run() -> None:
    if not sys.stdin.isatty():
        print("This TUI must be run in a real terminal.")
        return
    os.environ.setdefault("NCURSES_NO_UTF8_ACS", "1")
    curses.wrapper(main)


if __name__ == "__main__":
    run()
