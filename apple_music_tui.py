#!/usr/bin/env python3
import curses
import subprocess
import sys
import time
from dataclasses import dataclass, field
from typing import List, Tuple

APP_NAME = "Music"
POLL_INTERVAL = 1.0


@dataclass
class TrackInfo:
    name: str = ""
    artist: str = ""
    album: str = ""
    state: str = "STOPPED"
    duration: float = 0.0
    position: float = 0.0


@dataclass
class AppState:
    playlists: List[str] = field(default_factory=list)
    selected_index: int = 0
    now_playing: TrackInfo = field(default_factory=TrackInfo)
    status: str = ""
    last_poll: float = 0.0
    last_position_time: float = 0.0
    controls: dict = field(default_factory=dict)


def run_applescript(script: str) -> Tuple[str, str, int]:
    proc = subprocess.Popen(
        ["/usr/bin/osascript", "-e", script],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    out, err = proc.communicate()
    return out.strip(), err.strip(), proc.returncode


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
        state.status = err_msg
        return
    if code != 0:
        state.status = "AppleScript failed."
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
        state.status = err_msg
        return
    if code != 0:
        state.status = "AppleScript failed."
        return
    if out in ("NOT_RUNNING", ""):
        state.playlists = []
        state.selected_index = 0
        state.status = "Music app is not running."
        return
    playlists = [p.strip() for p in out.split("\n") if p.strip()]
    state.playlists = playlists
    if state.selected_index >= len(playlists):
        state.selected_index = max(0, len(playlists) - 1)
    state.status = f"Loaded {len(playlists)} playlists."


def play_pause(state: AppState) -> None:
    out, err, code = run_applescript(f'tell application "{APP_NAME}" to playpause')
    err_msg = format_error(err)
    if err_msg:
        state.status = err_msg
    elif code != 0:
        state.status = "AppleScript failed."
    else:
        state.status = "Toggled play/pause."


def next_track(state: AppState) -> None:
    out, err, code = run_applescript(f'tell application "{APP_NAME}" to next track')
    err_msg = format_error(err)
    if err_msg:
        state.status = err_msg
    elif code != 0:
        state.status = "AppleScript failed."
    else:
        state.status = "Next track."


def previous_track(state: AppState) -> None:
    out, err, code = run_applescript(f'tell application "{APP_NAME}" to previous track')
    err_msg = format_error(err)
    if err_msg:
        state.status = err_msg
    elif code != 0:
        state.status = "AppleScript failed."
    else:
        state.status = "Previous track."


def play_selected_playlist(state: AppState) -> None:
    if not state.playlists:
        state.status = "No playlists found."
        return
    name = applescript_escape(state.playlists[state.selected_index])
    script = f'''
    tell application "{APP_NAME}"
        play playlist "{name}"
    end tell
    '''
    out, err, code = run_applescript(script)
    err_msg = format_error(err)
    if err_msg:
        state.status = err_msg
    elif code != 0:
        state.status = "AppleScript failed."
    else:
        state.status = f"Playing playlist: {name}"


def draw_header(stdscr, width, state: AppState, colors) -> None:
    if width <= 0:
        return
    header = "  Apple Music TUI  "
    status = state.now_playing.state or "UNKNOWN"
    badge = f" {status} "
    stdscr.attron(curses.color_pair(colors["header"]))
    if width == 1:
        stdscr.addstr(0, 0, " ")
    else:
        stdscr.addstr(0, 0, " " * (width - 1))
        stdscr.addstr(0, 1, header[: max(0, width - 2)])
    if width > 1 and width - len(badge) - 3 > 0:
        stdscr.attron(curses.color_pair(colors["accent"]))
        stdscr.addstr(0, width - len(badge) - 3, "●")
        stdscr.attroff(curses.color_pair(colors["accent"]))
        stdscr.addstr(0, width - len(badge) - 2, badge)
    stdscr.attroff(curses.color_pair(colors["header"]))


def draw_box(stdscr, y, x, h, w, title, colors) -> None:
    if h < 3 or w < 4:
        return
    stdscr.attron(curses.color_pair(colors["border"]))
    if title and w > 6:
        label = f" {title} "
        available = w - 2
        trimmed = label[:available]
        left = max(1, (available - len(trimmed)) // 2)
        right = available - len(trimmed) - left
        top = "╔" + ("═" * left) + trimmed + ("═" * right) + "╗"
        stdscr.addstr(y, x, top[:w])
    else:
        stdscr.addstr(y, x, "╔" + "═" * (w - 2) + "╗")
    for row in range(1, h - 1):
        stdscr.addstr(y + row, x, "║")
        stdscr.addstr(y + row, x + w - 1, "║")
    stdscr.addstr(y + h - 1, x, "╚" + "═" * (w - 2) + "╝")
    stdscr.attroff(curses.color_pair(colors["border"]))


def draw_now_playing(stdscr, y, x, h, w, state: AppState, colors) -> None:
    draw_box(stdscr, y, x, h, w, "Now Playing", colors)
    if h < 7 or w < 12:
        return
    content_y = y + 1
    content_x = x + 2
    max_text_w = w - 4
    info = state.now_playing
    if info.state == "NOT_RUNNING":
        stdscr.addstr(content_y, content_x, "Music app is not running."[:max_text_w])
        return
    if info.state in ("STOPPED", "UNKNOWN", ""):
        stdscr.addstr(content_y, content_x, "Nothing playing."[:max_text_w])
        return
    stdscr.attron(curses.A_BOLD)
    stdscr.addstr(content_y, content_x, (info.name or "Untitled")[:max_text_w])
    stdscr.attroff(curses.A_BOLD)
    line = content_y + 1
    if line < y + h - 1:
        meta = info.artist
        if info.album:
            meta = f"{info.artist}  |  {info.album}"
        stdscr.addstr(line, content_x, meta[:max_text_w])
    line += 1
    if line < y + h - 1:
        state_text = f"State: {info.state}"
        stdscr.addstr(line, content_x, state_text[:max_text_w])
    line += 1
    if line < y + h - 1:
        progress_line = format_progress_line(max_text_w, info.position, info.duration)
        stdscr.addstr(line, content_x, progress_line[:max_text_w])


def draw_controls_panel(stdscr, y, x, h, w, state: AppState, colors) -> None:
    draw_box(stdscr, y, x, h, w, "Controls", colors)
    if h < 5 or w < 20:
        return
    controls = [
        ("Prev", "⟲ p"),
        ("Play", "▶ o"),
        ("Pause", "Ⅱ a"),
        ("Stop", "■ s"),
        ("Next", "n ⟳"),
    ]
    content_y = y + 1
    content_x = x + 2
    max_text_w = w - 4
    stdscr.attron(curses.A_BOLD)
    cursor_x = content_x
    state.controls = {}
    for name, label in controls:
        text = f"[{label}]"
        if cursor_x + len(text) < x + w - 1:
            stdscr.addstr(content_y, cursor_x, text[:max_text_w])
            state.controls[name] = (content_y, cursor_x, len(text))
        cursor_x += len(text) + 1
    stdscr.attroff(curses.A_BOLD)
    hint = "space: toggle   r: refresh   click buttons"
    if content_y + 1 < y + h - 1:
        stdscr.addstr(content_y + 1, content_x, hint[:max_text_w])


def draw_shortcuts_panel(stdscr, y, x, h, w, colors) -> None:
    draw_box(stdscr, y, x, h, w, "Shortcuts", colors)
    if h < 5 or w < 20:
        return
    lines = [
        "j/k: move   Enter: play",
        "n/p: next/prev",
        "o/a/s: play/pause/stop",
        "q: quit",
    ]
    content_y = y + 1
    content_x = x + 2
    max_text_w = w - 4
    for idx, line in enumerate(lines):
        row = content_y + idx
        if row >= y + h - 1:
            break
        stdscr.addstr(row, content_x, line[:max_text_w])


def draw_stats_panel(stdscr, y, x, h, w, state: AppState, colors) -> None:
    draw_box(stdscr, y, x, h, w, "Session", colors)
    if h < 5 or w < 20:
        return
    info = state.now_playing
    lines = [
        f"Playlists: {len(state.playlists)}",
        f"Poll: {POLL_INTERVAL:.1f}s",
        f"Position: {format_time(info.position)}",
        f"Duration: {format_time(info.duration)}",
    ]
    content_y = y + 1
    content_x = x + 2
    max_text_w = w - 4
    for idx, line in enumerate(lines):
        row = content_y + idx
        if row >= y + h - 1:
            break
        stdscr.addstr(row, content_x, line[:max_text_w])


def draw_playlists(stdscr, y, x, h, w, state: AppState, colors) -> None:
    title = f"Playlists ({len(state.playlists)})"
    draw_box(stdscr, y, x, h, w, title, colors)
    if h < 5 or w < 10:
        return
    content_y = y + 1
    content_x = x + 2
    max_rows = h - 2
    max_text_w = w - 4

    if not state.playlists:
        stdscr.addstr(content_y, content_x, "No playlists found."[:max_text_w])
        return

    start = max(0, state.selected_index - max_rows + 1)
    visible = state.playlists[start : start + max_rows]
    for idx, name in enumerate(visible):
        row = content_y + idx
        if row >= y + h - 1:
            break
        if start + idx == state.selected_index:
            stdscr.attron(curses.color_pair(colors["selected"]))
            stdscr.addstr(row, content_x, name[:max_text_w])
            stdscr.attroff(curses.color_pair(colors["selected"]))
        else:
            stdscr.addstr(row, content_x, name[:max_text_w])


def draw_status(stdscr, y, width, state: AppState, colors) -> None:
    if y < 0 or width <= 0:
        return
    status = state.status or "Ready."
    stdscr.attron(curses.color_pair(colors["status"]))
    if width == 1:
        stdscr.addstr(y, 0, " ")
    else:
        stdscr.addstr(y, 0, " " * (width - 1))
        stdscr.addstr(y, 1, status[: max(0, width - 2)])
    stdscr.attroff(curses.color_pair(colors["status"]))


def init_colors() -> dict:
    curses.start_color()
    curses.use_default_colors()
    curses.init_pair(1, curses.COLOR_BLACK, curses.COLOR_CYAN)
    curses.init_pair(2, curses.COLOR_WHITE, -1)
    curses.init_pair(3, curses.COLOR_BLACK, curses.COLOR_CYAN)
    curses.init_pair(4, curses.COLOR_CYAN, -1)
    curses.init_pair(5, curses.COLOR_WHITE, curses.COLOR_BLUE)
    curses.init_pair(6, curses.COLOR_CYAN, curses.COLOR_BLACK)

    return {
        "header": 1,
        "border": 4,
        "selected": 3,
        "status": 5,
        "accent": 6,
    }


def draw_ui(stdscr, state: AppState, colors) -> None:
    stdscr.erase()
    height, width = stdscr.getmaxyx()
    draw_header(stdscr, width, state, colors)

    content_top = 1
    content_height = height - 3
    status_row = height - 2

    if content_height < 6:
        draw_status(stdscr, status_row, width, state, colors)
        stdscr.refresh()
        return

    now_h = 8 if content_height >= 8 else max(5, content_height // 2)
    if content_height - now_h < 5:
        draw_now_playing(stdscr, content_top, 0, content_height, width, state, colors)
    else:
        draw_now_playing(stdscr, content_top, 0, now_h, width, state, colors)
        bottom_top = content_top + now_h
        bottom_h = content_height - now_h

        if width < 90:
            draw_playlists(stdscr, bottom_top, 0, bottom_h, width, state, colors)
        else:
            left_w = width * 2 // 3
            right_w = width - left_w
            draw_playlists(stdscr, bottom_top, 0, bottom_h, left_w, state, colors)

            if bottom_h >= 15:
                base = bottom_h // 3
                extra = bottom_h % 3
                controls_h = base + (1 if extra > 0 else 0)
                shortcuts_h = base + (1 if extra > 1 else 0)
                stats_h = bottom_h - controls_h - shortcuts_h
                draw_controls_panel(stdscr, bottom_top, left_w, controls_h, right_w, state, colors)
                draw_shortcuts_panel(
                    stdscr, bottom_top + controls_h, left_w, shortcuts_h, right_w, colors
                )
                draw_stats_panel(
                    stdscr,
                    bottom_top + controls_h + shortcuts_h,
                    left_w,
                    stats_h,
                    right_w,
                    state,
                    colors,
                )
            elif bottom_h >= 10:
                controls_h = bottom_h // 2
                shortcuts_h = bottom_h - controls_h
                draw_controls_panel(stdscr, bottom_top, left_w, controls_h, right_w, state, colors)
                draw_shortcuts_panel(
                    stdscr, bottom_top + controls_h, left_w, shortcuts_h, right_w, colors
                )
            else:
                draw_controls_panel(stdscr, bottom_top, left_w, bottom_h, right_w, state, colors)

    draw_status(stdscr, status_row, width, state, colors)
    stdscr.refresh()


def handle_key(stdscr, state: AppState, key: int) -> bool:
    if key in (ord("q"), ord("Q")):
        return False
    if key in (curses.KEY_DOWN, ord("j")):
        if state.selected_index < len(state.playlists) - 1:
            state.selected_index += 1
    elif key in (curses.KEY_UP, ord("k")):
        if state.selected_index > 0:
            state.selected_index -= 1
    elif key in (curses.KEY_ENTER, 10, 13):
        play_selected_playlist(state)
    elif key == ord(" "):
        play_pause(state)
    elif key in (ord("n"), ord("N")):
        next_track(state)
    elif key in (ord("p"), ord("P")):
        previous_track(state)
    elif key in (ord("o"), ord("O")):
        play_track(state)
    elif key in (ord("a"), ord("A")):
        pause_track(state)
    elif key in (ord("s"), ord("S")):
        stop_track(state)
    elif key in (ord("r"), ord("R")):
        fetch_playlists(state)
        fetch_now_playing(state)
    elif key == curses.KEY_MOUSE:
        try:
            _, mx, my, _, mouse_state = curses.getmouse()
        except curses.error:
            return True
        if mouse_state & curses.BUTTON1_CLICKED:
            for name, (y, x, w) in state.controls.items():
                if my == y and x <= mx < x + w:
                    if name == "Prev":
                        previous_track(state)
                    elif name == "Next":
                        next_track(state)
                    elif name == "Play":
                        play_track(state)
                    elif name == "Pause":
                        pause_track(state)
                    elif name == "Stop":
                        stop_track(state)
    return True


def main(stdscr) -> None:
    curses.curs_set(0)
    stdscr.nodelay(True)
    stdscr.keypad(True)
    curses.mousemask(curses.ALL_MOUSE_EVENTS)
    colors = init_colors()

    state = AppState()
    fetch_playlists(state)
    fetch_now_playing(state)
    state.last_poll = time.time()

    running = True
    while running:
        now = time.time()
        if now - state.last_poll >= POLL_INTERVAL:
            fetch_now_playing(state)
            state.last_poll = now
        if state.now_playing.state == "PLAYING" and state.now_playing.duration > 0:
            if state.last_position_time:
                delta = max(0.0, now - state.last_position_time)
                state.now_playing.position = min(
                    state.now_playing.duration, state.now_playing.position + delta
                )
            state.last_position_time = now
        draw_ui(stdscr, state, colors)
        key = stdscr.getch()
        if key != -1:
            running = handle_key(stdscr, state, key)
        time.sleep(0.03)


def run() -> None:
    if not sys.stdin.isatty():
        print("This TUI must be run in a real terminal.")
        return
    curses.wrapper(main)


def play_track(state: AppState) -> None:
    out, err, code = run_applescript(f'tell application "{APP_NAME}" to play')
    err_msg = format_error(err)
    if err_msg:
        state.status = err_msg
    elif code != 0:
        state.status = "AppleScript failed."
    else:
        state.status = "Play."


def pause_track(state: AppState) -> None:
    out, err, code = run_applescript(f'tell application "{APP_NAME}" to pause')
    err_msg = format_error(err)
    if err_msg:
        state.status = err_msg
    elif code != 0:
        state.status = "AppleScript failed."
    else:
        state.status = "Pause."


def stop_track(state: AppState) -> None:
    out, err, code = run_applescript(f'tell application "{APP_NAME}" to stop')
    err_msg = format_error(err)
    if err_msg:
        state.status = err_msg
    elif code != 0:
        state.status = "AppleScript failed."
    else:
        state.status = "Stop."


def format_time(seconds: float) -> str:
    if seconds <= 0:
        return "--:--"
    total = int(seconds)
    minutes = total // 60
    secs = total % 60
    return f"{minutes}:{secs:02d}"


def format_progress_bar(width: int, position: float, duration: float) -> str:
    if width <= 0:
        return ""
    if duration <= 0:
        return "-" * width
    ratio = max(0.0, min(1.0, position / duration))
    filled = int(ratio * width)
    if filled <= 0:
        return ">" + "-" * (width - 1)
    if filled >= width:
        return "=" * width
    return "=" * (filled - 1) + ">" + "-" * (width - filled)


def format_progress_line(max_width: int, position: float, duration: float) -> str:
    time_text = f"{format_time(position)} / {format_time(duration)}"
    if max_width <= len(time_text) + 2:
        return time_text[:max_width]
    bar_width = max_width - len(time_text) - 1
    if bar_width < 10:
        return time_text[:max_width]
    inner = max(1, bar_width - 2)
    if duration <= 0:
        bar_inner = "·" * inner
    else:
        ratio = max(0.0, min(1.0, position / duration))
        filled = ratio * inner
        full = int(filled)
        rem = filled - full
        partials = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"]
        part_index = int(rem * 8)
        part = partials[part_index]
        if full >= inner:
            bar_inner = "█" * inner
        else:
            bar_inner = ("█" * full) + part + (" " * max(0, inner - full - (1 if part else 0)))
    bar = f"▏{bar_inner}▕"
    return f"{bar} {time_text}"[:max_width]


if __name__ == "__main__":
    run()
