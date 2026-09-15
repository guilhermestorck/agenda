#!/usr/bin/env python3
"""Emit SVG mockups of agenda's screens, for importing into Penpot as editable shapes.

Reproducible on purpose: the design is generated from one description of the app's real
palette and layout, so when the implementation moves the mockups can be regenerated rather
than redrawn. Penpot turns each SVG into shapes you can edit by hand.
"""
import pathlib

OUT = pathlib.Path(__file__).resolve().parent.parent / "design"

# Adwaita dark, as the application actually renders it.
BG        = "#1d1d20"
SURFACE   = "#242428"
HEADER    = "#2e2e32"
LINE      = "#ffffff1f"
TEXT      = "#ffffff"
DIM       = "#ffffff80"
ACCENT    = "#78aeed"
ERROR     = "#ff7b63"
W, H      = 1160, 800
SIDEBAR   = 280
HEADER_H  = 46

ACCOUNTS = [
    ("work",     "#e66100", "W", ["Guilherme (work)", "On-call rota", "Team platform", "Travel"]),
    ("personal", "#3584e4", "P", ["Personal", "Birthdays", "Health", "Holidays in Spain"]),
    ("side",     "#33d17a", "S", ["Side projects", "Clients", "OSS releases"]),
]

def esc(text):
    return text.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")

def rect(x, y, w, h, fill, rx=0, stroke=None, opacity=1.0):
    return {"kind": "rect", "x": x, "y": y, "w": w, "h": h,
            "fill": fill, "rx": rx, "stroke": stroke, "opacity": opacity}

def text(x, y, body, fill=TEXT, size=12, weight=400, anchor="start"):
    return {"kind": "text", "x": x, "y": y, "body": body,
            "fill": fill, "size": size, "weight": weight, "anchor": anchor}

def line(x1, y1, x2, y2, stroke=LINE):
    """A hairline. A rect one pixel thick, so both renderers can draw it."""
    horizontal = y1 == y2
    return rect(x1, y1, (x2 - x1) if horizontal else 1, 1 if horizontal else (y2 - y1), stroke)

def circle(cx, cy, r, fill):
    return {"kind": "ellipse", "x": cx - r, "y": cy - r, "w": r * 2, "h": r * 2, "fill": fill}

def to_svg(item):
    if item["kind"] == "ellipse":
        return (f'<ellipse cx="{item["x"] + item["w"] / 2}" cy="{item["y"] + item["h"] / 2}" '
                f'rx="{item["w"] / 2}" ry="{item["h"] / 2}" fill="{item["fill"]}"/>')
    if item["kind"] == "rect":
        stroke = f' stroke="{item["stroke"]}"' if item["stroke"] else ""
        opacity = "" if item["opacity"] == 1.0 else f' opacity="{item["opacity"]}"'
        return (f'<rect x="{item["x"]}" y="{item["y"]}" width="{item["w"]}" '
                f'height="{item["h"]}" rx="{item["rx"]}" fill="{item["fill"]}"{stroke}{opacity}/>')
    return (f'<text x="{item["x"]}" y="{item["y"]}" fill="{item["fill"]}" '
            f'font-family="Noto Sans, sans-serif" font-size="{item["size"]}" '
            f'font-weight="{item["weight"]}" text-anchor="{item["anchor"]}">'
            f'{esc(item["body"])}</text>')

def chrome(title, span_label="Week"):
    """Header bar shared by every main view."""
    out = [rect(0, 0, W, H, BG), rect(0, 0, W, HEADER_H, HEADER)]
    out.append(text(24, 29, "⟳", DIM, 15))
    out.append(text(56, 29, "◧", DIM, 15))
    out.append(text(W // 2, 29, title, TEXT, 13, 500, "middle"))
    out.append(rect(742, 10, 84, 26, SURFACE, 6))
    out.append(text(752, 28, span_label, TEXT, 12))
    out.append(text(816, 28, "▾", DIM, 10))
    out.append(rect(836, 10, 66, 26, SURFACE, 6))
    out.append(text(848, 28, "Today", TEXT, 12))
    out.append(rect(910, 10, 64, 26, SURFACE, 6))
    out.append(text(926, 28, "‹", TEXT, 14))
    out.append(text(958, 28, "›", TEXT, 14))
    out.append(text(716, 29, "☰", DIM, 14))
    return out

def sidebar(hidden_calendar=None):
    """View-only: swatch, avatar, name, and the hover eye."""
    out = [rect(0, HEADER_H, SIDEBAR, H - HEADER_H, SURFACE)]
    y = HEADER_H + 40
    for name, color, initial, calendars in ACCOUNTS:
        out.append(rect(20, y - 11, 14, 14, color, 3))
        out.append(circle(52, y - 4, 11, color))
        out.append(text(52, y, initial, "#ffffff", 11, 500, "middle"))
        out.append(text(72, y, name, TEXT, 13, 500))
        out.append(text(252, y, "\U0001f441", DIM, 12))
        y += 22
        out.append(text(72, y, "Needs reconnecting", ERROR, 11))
        y += 26
        for calendar in calendars:
            dim = calendar == hidden_calendar
            out.append(rect(24, y - 10, 12, 12, color, 3))
            out.append(text(48, y, calendar, DIM if dim else TEXT, 12))
            if dim:
                out.append(text(252, y, "\U0001f441", DIM, 12))
            y += 26
        y += 16
    return out

def week_view():
    out = chrome("14 – 20 September 2026")
    out += sidebar("Travel")
    days = ["Mon 14", "Tue 15", "Wed 16", "Thu 17", "Fri 18", "Sat 19", "Sun 20"]
    grid_x, grid_w = SIDEBAR + 56, W - SIDEBAR - 56
    col = grid_w / 7
    for index, day in enumerate(days):
        x = grid_x + index * col
        out.append(text(x + col / 2, HEADER_H + 26, day.split()[0], ACCENT if index == 1 else TEXT, 12, 500, "middle"))
        out.append(text(x + col / 2, HEADER_H + 42, day.split()[1] + " Sep", DIM, 10, 400, "middle"))
        out.append(line(x, HEADER_H + 54, x, H))
    out.append(rect(SIDEBAR, HEADER_H + 54, W - SIDEBAR, 44, BG))
    out.append(rect(grid_x + 4, HEADER_H + 62, col - 8, 16, "#3584e4", 3))
    out.append(text(grid_x + 10, HEADER_H + 74, "Fiesta local", "#ffffff", 9))

    # Compressed quiet hours above, full height inside the core band.
    y = HEADER_H + 110
    for hour in range(7, 20):
        tall = 8 <= hour < 22
        height = 40 if tall else 20
        out.append(line(SIDEBAR, y, W, y))
        out.append(text(SIDEBAR + 44, y + 12, f"{hour:02}:00", DIM, 10, 400, "end"))
        y += height
    events = [(0, 150, 34, "Sprint planning", "#e66100"), (1, 120, 26, "Design review", "#e66100"),
              (2, 190, 30, "CI window (UTC)", "#33d17a"), (4, 240, 26, "Client call", "#33d17a"),
              (1, 300, 22, "Piano lesson", "#c061cb")]
    for day_index, offset, height, label, color in events:
        x = grid_x + day_index * col + 3
        top = HEADER_H + 110 + offset
        out.append(rect(x, top - 10, col - 8, 10, color, 3, opacity=0.28))
        out.append(rect(x, top, col - 8, height, color, 3))
        out.append(text(x + 6, top + 13, label, "#ffffff", 10))
    return out

def month_view():
    out = chrome("September 2026", "Month")
    out += sidebar()
    grid_x, grid_w = SIDEBAR, W - SIDEBAR
    col, row_h = grid_w / 7, (H - HEADER_H - 30) / 6
    for index, day in enumerate(["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]):
        out.append(text(grid_x + index * col + col / 2, HEADER_H + 20, day, DIM, 11, 400, "middle"))
    number = 31
    for week in range(6):
        top = HEADER_H + 30 + week * row_h
        out.append(line(grid_x, top, W, top))
        for day in range(7):
            x = grid_x + day * col
            outside = week == 0 and day == 0
            out.append(text(x + 8, top + 16, str(number), DIM if outside else TEXT, 10))
            for chip in range(min(3, (week + day) % 4)):
                colour = ACCOUNTS[chip % 3][1]
                out.append(rect(x + 6, top + 24 + chip * 15, 3, 11, colour))
                out.append(text(x + 13, top + 33 + chip * 15, ["Standup", "Gym", "Block 1"][chip], TEXT, 9))
            if (week + day) % 4 == 3:
                out.append(text(x + 8, top + 72, "+3 more", DIM, 9))
            number = 1 if number >= 30 else number + 1
    return out

def agenda_view():
    out = chrome("15 Sep – 15 Oct 2026", "Agenda")
    out += sidebar()
    x, y = SIDEBAR + 24, HEADER_H + 34
    days = [("Today — Tuesday 15 September", [("09:15", "Daily standup", "#e66100"),
                                                   ("10:00", "Design review", "#e66100"),
                                                   ("10:30", "Dentist", "#3584e4")]),
            ("Wednesday 16 September", [("All day", "Fiesta local", "#3584e4"),
                                        ("07:00", "Gym", "#3584e4"),
                                        ("11:00", "Concurrent block 1", "#33d17a")])]
    for heading, rows in days:
        out.append(text(x, y, heading, ACCENT if heading.startswith("Today") else TEXT, 13, 500))
        y += 26
        for when, summary, colour in rows:
            out.append(text(x, y, when, DIM, 11))
            out.append(rect(x + 70, y - 10, 3, 13, colour))
            out.append(text(x + 82, y, summary, TEXT, 12))
            y += 24
        y += 18
    return out

def preferences(page):
    """The dialog, one page at a time."""
    w, h = 720, 620
    out = [rect(0, 0, w, h, BG), rect(0, 0, w, 46, HEADER)]
    out.append(text(w // 2, 28, "Preferences", TEXT, 13, 500, "middle"))
    for index, (label, active) in enumerate([("Settings", page == "settings"), ("Accounts", page == "accounts")]):
        x = w // 2 - 100 + index * 100
        out.append(rect(x, 56, 92, 28, SURFACE if active else BG, 6))
        out.append(text(x + 46, 75, label, TEXT if active else DIM, 12, 500 if active else 400, "middle"))
    y = 116
    if page == "settings":
        groups = [("Time zones", [("Display time zone", "Europe/Madrid"),
                                  ("Second time zone", "America/New_York")]),
                  ("Working hours", [("Day starts", "8"), ("Day ends", "22")])]
    else:
        groups = [("work — work@example.com", [(c, "") for c in ACCOUNTS[0][3]]),
                  ("personal — personal@example.com", [(c, "") for c in ACCOUNTS[1][3]])]
    for title, rows in groups:
        out.append(text(40, y, title, TEXT, 12, 500))
        y += 16
        out.append(rect(32, y, w - 64, len(rows) * 46 + 8, SURFACE, 10))
        y += 12
        for label, value in rows:
            out.append(text(48, y + 18, label, TEXT, 12))
            if value:
                out.append(text(w - 64, y + 18, value, DIM, 12, 400, "end"))
                out.append(text(w - 48, y + 18, "▾", DIM, 9))
            else:
                out.append(rect(w - 96, y + 6, 16, 16, ACCOUNTS[0][1], 3))
                out.append(text(w - 64, y + 18, "⏰", DIM, 11))
            y += 46
        y += 26
    return out, w, h

SCREENS = {}

def write(name, body, width=W, height=H):
    SCREENS[name] = {"width": width, "height": height, "items": body}
    svg = (f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" '
           f'viewBox="0 0 {width} {height}">' + "".join(to_svg(item) for item in body) + "</svg>")
    path = OUT / f"{name}.svg"
    path.write_text(svg)
    print(f"  {path.relative_to(OUT.parent)}  ({len(svg)} bytes)")

OUT.mkdir(exist_ok=True)
print("wrote:")
write("01-week", week_view())
write("02-month", month_view())
write("03-agenda", agenda_view())
body, w, h = preferences("settings")
write("04-preferences-settings", body, w, h)
body, w, h = preferences("accounts")
write("05-preferences-accounts", body, w, h)

# The same screens as a scene description, for the Penpot renderer to build natively.
import json
(OUT / "screens.json").write_text(json.dumps(SCREENS, indent=1))
print(f"  design/screens.json  ({len(SCREENS)} screens)")
