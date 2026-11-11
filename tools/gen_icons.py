#!/usr/bin/env python3
"""Gera os ícones SVG do vibrance-kde em ../assets/icons.

* vibrance-kde.svg                   ícone colorido do aplicativo
* vibrance-kde-symbolic.svg          bandeja: observador ligado (monocromático)
* vibrance-kde-active-symbolic.svg   bandeja: jogo em foco, vibrance aplicado
* vibrance-kde-disabled-symbolic.svg bandeja: observador desligado

Os ícones da bandeja seguem a convenção do Breeze: uma folha de estilo
`current-color-scheme` e `class="ColorScheme-Text"`, o que permite ao Plasma
recolorir o ícone conforme o tema (claro/escuro).
"""
import math
import os

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "assets", "icons")
os.makedirs(OUT, exist_ok=True)


def pt(cx, cy, r, deg):
    a = math.radians(deg)
    return cx + r * math.cos(a), cy + r * math.sin(a)


def sector(cx, cy, r_in, r_out, a0, a1):
    """Setor anular preenchido, de a0 a a1 graus (sentido horário, 0° = direita)."""
    x0, y0 = pt(cx, cy, r_out, a0)
    x1, y1 = pt(cx, cy, r_out, a1)
    x2, y2 = pt(cx, cy, r_in, a1)
    x3, y3 = pt(cx, cy, r_in, a0)
    large = 1 if (a1 - a0) % 360 > 180 else 0
    return (
        f"M{x0:.3f} {y0:.3f}"
        f"A{r_out} {r_out} 0 {large} 1 {x1:.3f} {y1:.3f}"
        f"L{x2:.3f} {y2:.3f}"
        f"A{r_in} {r_in} 0 {large} 0 {x3:.3f} {y3:.3f}Z"
    )


def ring(cx, cy, r_in, r_out, n, gap_deg, start=-90):
    step = 360 / n
    return [sector(cx, cy, r_in, r_out, start + i * step + gap_deg / 2, start + (i + 1) * step - gap_deg / 2)
            for i in range(n)]


def half_disc(cx, cy, r, left=True):
    """Meio disco (metade esquerda ou direita)."""
    top = (cx, cy - r)
    bot = (cx, cy + r)
    sweep = 0 if left else 1
    return f"M{top[0]} {top[1]}A{r} {r} 0 0 {sweep} {bot[0]} {bot[1]}Z"


def disc(cx, cy, r):
    return f"M{cx - r} {cy}A{r} {r} 0 1 1 {cx + r} {cy}A{r} {r} 0 1 1 {cx - r} {cy}Z"


STYLE = (
    '<style type="text/css" id="current-color-scheme">'
    ".ColorScheme-Text{color:#232629;}</style>"
)


def symbolic(name, body):
    svg = (
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 22 22" width="22" height="22">'
        f"<defs>{STYLE}</defs>{body}</svg>\n"
    )
    with open(os.path.join(OUT, name), "w") as f:
        f.write(svg)


def path(d, opacity=None):
    op = f' opacity="{opacity}"' if opacity else ""
    return f'<path class="ColorScheme-Text" fill="currentColor"{op} d="{d}"/>'


# --- ícones monocromáticos da bandeja (22x22) --------------------------------
C = 11
ring_paths = ring(C, C, 6.6, 9.8, 6, 7)

# observador ligado: anel + metade esquerda do disco (ideia de "antes/depois")
symbolic(
    "vibrance-kde-symbolic.svg",
    "".join(path(d) for d in ring_paths) + path(half_disc(C, C, 3.6, left=True)),
)

# jogo em foco: anel + disco cheio
symbolic(
    "vibrance-kde-active-symbolic.svg",
    "".join(path(d) for d in ring_paths) + path(disc(C, C, 3.6)),
)

# desligado: anel esmaecido + barra diagonal
slash = "M4.2 3.4L18.6 17.8L17.4 19L3 4.6Z"
symbolic(
    "vibrance-kde-disabled-symbolic.svg",
    "".join(path(d, 0.45) for d in ring_paths) + path(slash),
)

# --- ícone colorido do aplicativo (128x128) ----------------------------------
colors = ["#ff3b5c", "#ff9f1a", "#ffd60a", "#30d158", "#0ab4ff", "#bf5af2"]
cx = cy = 64
segs = ring(cx, cy, 30, 48, 6, 5)
ring_svg = "".join(f'<path fill="{c}" d="{d}"/>' for c, d in zip(colors, segs))

app = f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 128 128" width="128" height="128">
<defs>
<linearGradient id="bg" x1="0" y1="0" x2="0" y2="1">
<stop offset="0" stop-color="#3a3f4f"/><stop offset="1" stop-color="#171a22"/>
</linearGradient>
<linearGradient id="vivid" x1="0" y1="0" x2="1" y2="1">
<stop offset="0" stop-color="#ffd60a"/><stop offset="1" stop-color="#ff3b5c"/>
</linearGradient>
</defs>
<rect x="4" y="4" width="120" height="120" rx="26" fill="url(#bg)"/>
<rect x="4.5" y="4.5" width="119" height="119" rx="25.5" fill="none" stroke="#ffffff" stroke-opacity="0.12"/>
{ring_svg}
<path fill="#8e8e93" d="{half_disc(cx, cy, 20, left=True)}"/>
<path fill="url(#vivid)" d="{half_disc(cx, cy, 20, left=False)}"/>
</svg>
"""
with open(os.path.join(OUT, "vibrance-kde.svg"), "w") as f:
    f.write(app)

print("ícones gerados em", os.path.normpath(OUT))
