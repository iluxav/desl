#!/usr/bin/env python3
"""Render a terminal capture as the README's picture.

Take the capture from a desl running in tmux, with text marks so the glyphs render without a
Nerd Font, then render it:

    tmux -L shot new-session -d -s shot -x 132 -y 34 "env DESL_ICONS=text desl"
    tmux -L shot capture-pane -p -e -J -t shot > frame.ansi
    python3 docs/tools/ansi2svg.py frame.ansi docs/desl.svg 132 34

Every run of text is pinned to its cell width, so the columns line up in whatever monospace
font the viewer has. The palette is Tokyo Night.
"""
import re
import sys
import unicodedata
from html import escape

# Tokyo Night, a dark palette that reads on GitHub's light and dark pages alike.
BG = "#1a1b26"
FG = "#c0caf5"
ANSI = [
    "#15161e", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#a9b1d6",
    "#414868", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#c0caf5",
]
CW, LH, FS = 8.4, 19.0, 14  # cell width, line height, font size in px
PAD = 18


def cube(n):
    if n < 16:
        return ANSI[n]
    if n < 232:
        n -= 16
        r, g, b = n // 36, (n // 6) % 6, n % 6
        to = lambda v: 0 if v == 0 else 55 + v * 40
        return "#%02x%02x%02x" % (to(r), to(g), to(b))
    v = 8 + (n - 232) * 10
    return "#%02x%02x%02x" % (v, v, v)


def width(ch):
    if unicodedata.combining(ch):
        return 0
    return 2 if unicodedata.east_asian_width(ch) in ("W", "F") else 1


class Style:
    def __init__(self):
        self.fg = None
        self.bg = None
        self.bold = False
        self.dim = False
        self.italic = False
        self.underline = False
        self.reverse = False

    def key(self):
        return (self.fg, self.bg, self.bold, self.dim, self.italic, self.underline, self.reverse)

    def copy(self):
        s = Style()
        s.__dict__.update(self.__dict__)
        return s


def apply(style, params):
    ps = [int(p) if p else 0 for p in params.split(";")] if params else [0]
    i = 0
    while i < len(ps):
        p = ps[i]
        if p == 0:
            style.__init__()
        elif p == 1:
            style.bold = True
        elif p == 2:
            style.dim = True
        elif p == 3:
            style.italic = True
        elif p == 4:
            style.underline = True
        elif p == 7:
            style.reverse = True
        elif p == 22:
            style.bold = style.dim = False
        elif p == 23:
            style.italic = False
        elif p == 24:
            style.underline = False
        elif p == 27:
            style.reverse = False
        elif 30 <= p <= 37:
            style.fg = ANSI[p - 30]
        elif 90 <= p <= 97:
            style.fg = ANSI[p - 90 + 8]
        elif 40 <= p <= 47:
            style.bg = ANSI[p - 40]
        elif 100 <= p <= 107:
            style.bg = ANSI[p - 100 + 8]
        elif p == 39:
            style.fg = None
        elif p == 49:
            style.bg = None
        elif p in (38, 48) and i + 1 < len(ps):
            target = "fg" if p == 38 else "bg"
            if ps[i + 1] == 5 and i + 2 < len(ps):
                setattr(style, target, cube(ps[i + 2]))
                i += 2
            elif ps[i + 1] == 2 and i + 4 < len(ps):
                setattr(style, target, "#%02x%02x%02x" % (ps[i + 2], ps[i + 3], ps[i + 4]))
                i += 4
        i += 1


SGR = re.compile(r"\x1b\[([0-9;]*)m")


def parse(text):
    """Rows of (char, style) cells."""
    rows = []
    for line in text.split("\n"):
        style = Style()
        cells = []
        pos = 0
        for m in SGR.finditer(line):
            for ch in line[pos : m.start()]:
                cells.append((ch, style.copy()))
            apply(style, m.group(1))
            pos = m.end()
        for ch in line[pos:]:
            cells.append((ch, style.copy()))
        cells = [(c, s) for c, s in cells if c >= " " or c == "\t"]
        rows.append(cells)
    return rows


def render(rows, cols, nrows):
    w = cols * CW + 2 * PAD
    h = nrows * LH + 2 * PAD
    out = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{w:.0f}" height="{h:.0f}" viewBox="0 0 {w:.0f} {h:.0f}" font-family="ui-monospace, SFMono-Regular, Menlo, Consolas, \'DejaVu Sans Mono\', monospace" font-size="{FS}">',
        f'<rect width="{w:.0f}" height="{h:.0f}" rx="10" fill="{BG}"/>',
    ]
    for y, cells in enumerate(rows[:nrows]):
        # Runs of equal style; a run's advance is pinned so columns line up in any font.
        x = 0
        runs = []
        for ch, st in cells:
            cw = width(ch)
            if cw == 0 and runs:
                runs[-1][2] += ch
                continue
            if runs and runs[-1][1].key() == st.key():
                runs[-1][2] += ch
                runs[-1][3] += cw
            else:
                runs.append([x, st, ch, cw])
            x += cw
        ty = PAD + y * LH + LH * 0.75
        for x0, st, text, ncells in runs:
            fg, bg = st.fg or FG, st.bg
            if st.reverse:
                fg, bg = (bg or BG), (st.fg or FG)
            px = PAD + x0 * CW
            if bg:
                out.append(f'<rect x="{px:.1f}" y="{PAD + y * LH:.1f}" width="{ncells * CW:.1f}" height="{LH:.1f}" fill="{bg}"/>')
            if not text.strip():
                continue
            attrs = [f'x="{px:.1f}"', f'y="{ty:.1f}"', f'fill="{fg}"', f'textLength="{ncells * CW:.1f}"', 'lengthAdjust="spacingAndGlyphs"']
            if st.bold:
                attrs.append('font-weight="bold"')
            if st.italic:
                attrs.append('font-style="italic"')
            if st.underline:
                attrs.append('text-decoration="underline"')
            if st.dim:
                attrs.append('opacity="0.6"')
            out.append(f'<text xml:space="preserve" {" ".join(attrs)}>{escape(text)}</text>')
    out.append("</svg>")
    return "\n".join(out)


def main():
    src, dst = sys.argv[1], sys.argv[2]
    text = open(src, encoding="utf-8").read().rstrip("\n")
    rows = parse(text)
    cols = int(sys.argv[3]) if len(sys.argv) > 3 else max(sum(width(c) for c, _ in r) for r in rows)
    nrows = int(sys.argv[4]) if len(sys.argv) > 4 else len(rows)
    open(dst, "w", encoding="utf-8").write(render(rows, cols, nrows))
    print(f"{dst}: {cols}x{nrows}")


if __name__ == "__main__":
    main()
