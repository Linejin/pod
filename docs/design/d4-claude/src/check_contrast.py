"""WCAG contrast audit for the D4 proposal tokens (same formula as tools/visual/contrast.py).

Text >= 4.5:1 (high contrast themes >= 7:1), icons / control borders / focus ring >= 3:1.
Writes ../contrast.json and ../tokens.json, exits 1 on any failure.
"""
import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).parent))
from tokens import DIRECTIONS, METRICS  # noqa: E402


def lum(hex_):
    v = int(hex_.lstrip('#')[:6], 16)
    out = []
    for shift in (16, 8, 0):
        c = ((v >> shift) & 255) / 255
        out.append(c / 12.92 if c <= .04045 else ((c + .055) / 1.055) ** 2.4)
    return .2126 * out[0] + .7152 * out[1] + .0722 * out[2]


def ratio(a, b):
    a, b = lum(a), lum(b)
    return (max(a, b) + .05) / (min(a, b) + .05)


SURFACES = ['app', 'sidebar', 'panel', 'raised', 'overlay', 'hover', 'selected', 'selected_quiet']
STATUS = ['working', 'attention', 'done', 'failed', 'idle']

results = []
for d, themes in DIRECTIONS.items():
    for theme, t in themes.items():
        text_min = 7 if theme.startswith('hc') else 4.5
        pairs = []
        for fg in ('text', 'text2', 'muted', 'accent_fg'):
            pairs += [(fg, bg, text_min if fg != 'accent_fg' else 4.5, 'text') for bg in SURFACES]
        # status colors are used for short labels (4.5) and icons (3.0)
        for s in STATUS:
            pairs += [(s, bg, 4.5, 'status label') for bg in ('app', 'sidebar', 'panel', 'raised', 'hover', 'selected')]
            pairs += [(s, s + '_bg', 4.5, 'badge') for _ in [0] if s + '_bg' in t]
        pairs += [('accent_fg', s + '_bg', 4.5, 'banner action') for s in ('working', 'attention', 'failed')]
        pairs += [('text', s + '_bg', text_min, 'banner text') for s in ('working', 'attention', 'failed', 'done')]
        pairs += [('on_accent', 'accent', 4.5, 'button'), ('on_accent', 'accent_hover', 4.5, 'button')]
        pairs += [('control', bg, 3, 'control border') for bg in ('app', 'panel', 'raised', 'overlay')]
        pairs += [('focus', bg, 3, 'focus ring') for bg in ('app', 'sidebar', 'panel', 'raised', 'overlay', 'selected')]
        pairs += [('term_fg', 'term_bg', 7 if theme.startswith('hc') else 4.5, 'terminal')]
        pairs += [('add_fg', 'add_bg', 4.5, 'diff'), ('del_fg', 'del_bg', 4.5, 'diff'),
                  ('text', 'add_bg', text_min, 'diff'), ('text', 'del_bg', text_min, 'diff')]
        for fg, bg, minimum, kind in pairs:
            r = ratio(t[fg], t[bg])
            results.append(dict(direction=d, theme=theme, kind=kind, fg=fg, bg=bg,
                                fg_hex=t[fg], bg_hex=t[bg], ratio=round(r, 2), minimum=minimum,
                                passed=r >= minimum))

out = pathlib.Path(__file__).resolve().parents[1]
(out / 'contrast.json').write_text(json.dumps(results, indent=1, ensure_ascii=False) + '\n', encoding='utf-8')
(out / 'tokens.json').write_text(json.dumps({'colors': DIRECTIONS, 'metrics': METRICS}, indent=1) + '\n', encoding='utf-8')
fail = [r for r in results if not r['passed']]
for d in DIRECTIONS:
    for theme in DIRECTIONS[d]:
        rows = [r for r in results if r['direction'] == d and r['theme'] == theme]
        low = min(rows, key=lambda r: r['ratio'] / r['minimum'])
        print(f"{d} {theme:9} {sum(r['passed'] for r in rows)}/{len(rows)} pass; tightest {low['fg']}/{low['bg']} {low['ratio']} (min {low['minimum']})")
for r in fail:
    print('FAIL', r['direction'], r['theme'], r['fg'], 'on', r['bg'], r['ratio'], '<', r['minimum'])
sys.exit(1 if fail else 0)
