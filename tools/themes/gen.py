"""Generate Pod's built-in style themes (crates/pod-ui/themes/*.json) from the design tokens.

python -I tools/themes/gen.py          (from the repository root; no network)

Sources (docs/10 §D5):
  * docs/design/d4/tokens.json                       Codex A (01-03) + the shared high-contrast palettes
  * docs/design/d4-claude/src/tokens.py              Claude A/B/C (04-06) and their metrics
  * docs/design/gallery/src/build.py  P, CODEX_VARIANTS   gallery samples 07-18
Token names follow docs/10 §D4.4 (bg.canvas, text.muted, ...). Contrast corrections live in FIXES
with a reason; the Rust test `pod_ui::themes::tests::builtin_contrast` checks the same pairs.
"""
import json
import pathlib
import sys

sys.dont_write_bytecode = True
ROOT = pathlib.Path(__file__).resolve().parents[2]
OUT = ROOT / 'crates' / 'pod-ui' / 'themes'
import importlib.util  # noqa: E402

# The gallery script imports the Claude renderer as `build`; load the gallery under another name.
_spec = importlib.util.spec_from_file_location('pod_gallery', ROOT / 'docs' / 'design' / 'gallery' / 'src' / 'build.py')
gallery = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(gallery)  # pure data at import; rendering is in its main()
from tokens import DIRECTIONS, METRICS  # noqa: E402  (d4-claude/src/tokens.py, put on sys.path by the gallery)

CODEX = json.loads((ROOT / 'docs' / 'design' / 'd4' / 'tokens.json').read_text(encoding='utf-8'))


def shade(hex_color, factor):
    """Multiply each sRGB channel (darken < 1)."""
    v = int(hex_color[1:7], 16)
    ch = [min(255, max(0, round(((v >> s) & 255) * factor))) for s in (16, 8, 0)]
    return '#%02X%02X%02X' % tuple(ch)


def from_claude(p, dark):
    """Claude/gallery role names -> docs/10 §D4.4 token names."""
    return {
        'bg.canvas': p['app'], 'bg.chrome': p['sidebar'], 'bg.surface': p['panel'], 'bg.raised': p['raised'],
        'bg.overlay': p['overlay'], 'bg.hover': p['hover'], 'bg.pressed': p['active'], 'bg.selected': p['selected'],
        'border.subtle': p['hairline'], 'border.strong': p['strong'], 'border.control': p['control'],
        'text.primary': p['text'], 'text.secondary': p['text2'], 'text.muted': p['muted'], 'text.disabled': p['disabled'],
        'accent.fg': p['accent_fg'], 'focus.ring': p['focus'],
        'action.primary.bg': p['accent'], 'action.primary.hover': p['accent_hover'],
        'action.primary.pressed': shade(p['accent_hover'], .9), 'action.primary.fg': p['on_accent'],
        'success.fg': p['done'], 'success.bg': p['done_bg'], 'warning.fg': p['attention'], 'warning.bg': p['attention_bg'],
        'danger.fg': p['failed'], 'danger.bg': p['failed_bg'], 'info.fg': p['working'], 'info.bg': p['working_bg'],
        'diff.add.bg': p['add_bg'], 'diff.add.fg': p['add_fg'], 'diff.del.bg': p['del_bg'], 'diff.del.fg': p['del_fg'],
        'terminal.bg': p['term_bg'], 'terminal.fg': p['term_fg'], 'terminal.selection': p['selected'],
        'terminal.cursor': p['accent_fg'], 'scrim': '#00000066' if dark else '#15203033',
    }


def from_codex(p, dark):
    """Codex A role names -> docs/10 §D4.4 token names."""
    return {
        'bg.canvas': p['app'], 'bg.chrome': p['sidebar'], 'bg.surface': p['panel'], 'bg.raised': p['raised'],
        'bg.overlay': p['overlay'], 'bg.hover': p['hover'], 'bg.pressed': p['pressed'], 'bg.selected': p['selected'],
        'border.subtle': p['hairline'], 'border.strong': p['control'], 'border.control': p['control'],
        'text.primary': p['primary'], 'text.secondary': p['secondary'], 'text.muted': p['muted'], 'text.disabled': p['disabled'],
        'accent.fg': p['accent'], 'focus.ring': p['accent'],
        'action.primary.bg': p['button'], 'action.primary.hover': p['button_hover'],
        'action.primary.pressed': p['button_pressed'], 'action.primary.fg': p['button_fg'],
        'success.fg': p['success'], 'success.bg': p['success_bg'], 'warning.fg': p['warning'], 'warning.bg': p['warning_bg'],
        'danger.fg': p['danger'], 'danger.bg': p['danger_bg'], 'info.fg': p['info'], 'info.bg': p['info_bg'],
        'diff.add.bg': p['success_bg'], 'diff.add.fg': p['success'], 'diff.del.bg': p['danger_bg'], 'diff.del.fg': p['danger'],
        'terminal.bg': p['terminal'], 'terminal.fg': p['terminal_fg'], 'terminal.selection': p['selected'],
        'terminal.cursor': p['cursor'], 'scrim': '#00000066' if dark else '#15203033',
    }


def codex(theme, over=None):
    p = dict(CODEX[theme])
    p.update(over or {})
    return from_codex(p, theme.endswith('dark'))


def claude(p, theme):
    return from_claude(p, theme.endswith('dark'))


def density(row, title, tab, panel, status, sidebar, ui=13):
    return {'row': row, 'titlebar': title, 'tabbar': tab, 'panel_header': panel, 'statusbar': status, 'sidebar': sidebar, 'ui_font': ui}


A, B, C = DIRECTIONS['A'], DIRECTIONS['B'], DIRECTIONS['C']
P, CV = gallery.P, gallery.CODEX_VARIANTS
SHARED_HC = {'hc-dark': codex('hc-dark'), 'hc-light': codex('hc-light')}
QUIET_HC = {'hc-dark': claude(A['hc-dark'], 'hc-dark'), 'hc-light': claude(A['hc-light'], 'hc-light')}
D_WORKBENCH = density(28, 36, 32, 28, 24, 240)
D_QUIET = density(METRICS['A']['row'], METRICS['A']['titlebar'], METRICS['A']['tabbar'], METRICS['A']['panel_header'], 24, 248)

# id, name, sample, source, surface, density, radius(control, card, overlay), gap, shadow, stroke, focus, border, accent override,
# dark, light, hc (None = shared)
STYLES = [
    ('workbench', '정밀한 작업대', '01', 'Codex A', 'flat', D_WORKBENCH, (4, 6, 8), 0, 'none', 2.0, 2, 1, True,
     codex('dark'), codex('light'), SHARED_HC),
    ('quiet', '조용한 편집기', '04', 'Claude A', 'flat', D_QUIET, (4, 6, 8), 0, 'none', 1.5, 2, 1, True,
     claude(A['dark'], 'dark'), claude(A['light'], 'light'), QUIET_HC),
    ('layered', '레이어형', '05', 'Claude B', 'layered', density(30, 40, 34, 32, 26, 256), (6, 8, 12), 6, 'soft', 1.5, 2, 1, True,
     claude(B['dark'], 'dark'), claude(B['light'], 'light'), SHARED_HC),
    ('mission', '관제형', '06', 'Claude C', 'layered', density(32, 36, 32, 32, 24, 280), (6, 8, 10), 4, 'none', 1.5, 2, 1, True,
     claude(C['dark'], 'dark'), claude(C['light'], 'light'), SHARED_HC),
    ('dense', '고밀도 편집기', '07', 'gallery dense', 'flat', density(22, 30, 26, 22, 22, 216, 12), (3, 4, 6), 0, 'none', 1.5, 1, 1, True,
     claude(P['dense']['dark'], 'dark'), claude(P['dense']['light'], 'light'), SHARED_HC),
    ('islands', '섬형', '08', 'gallery islands', 'islands', density(32, 44, 36, 36, 30, 236), (8, 12, 12), 10, 'raised', 1.5, 2, 1, True,
     claude(P['islands']['dark'], 'dark'), claude(P['islands']['light'], 'light'), SHARED_HC),
    ('mono', '미니멀 단색', '09', 'gallery mono', 'mono', density(30, 44, 32, 32, 24, 248), (4, 6, 8), 0, 'none', 1.5, 2, 0, False,
     claude(P['mono']['dark'], 'dark'), claude(P['mono']['light'], 'light'), SHARED_HC),
    ('warm', '따뜻한 중성색', '10', 'gallery warm', 'flat', density(32, 38, 32, 32, 24, 248), (6, 8, 10), 0, 'soft', 1.5, 2, 1, True,
     claude(P['warm']['dark'], 'dark'), claude(P['warm']['light'], 'light'), SHARED_HC),
    ('blocks', '블록 터미널', '11', 'gallery blocks', 'flat', D_QUIET, (4, 6, 8), 0, 'none', 1.5, 2, 1, True,
     claude(P['blocks']['dark'], 'dark'), claude(P['blocks']['light'], 'light'), QUIET_HC),
    ('kanban', '칸반 보드', '12', 'gallery kanban', 'layered', density(32, 36, 32, 32, 24, 248), (6, 8, 10), 6, 'soft', 1.5, 2, 1, True,
     claude(P['kanban']['dark'], 'dark'), claude(P['kanban']['light'], 'light'), SHARED_HC),
    ('review', '분할 리뷰 중심', '13', 'gallery review', 'flat', D_QUIET, (4, 6, 8), 0, 'none', 1.5, 2, 1, True,
     claude(P['review']['dark'], 'dark'), claude(P['review']['light'], 'light'), QUIET_HC),
    ('access', '고대비·접근성 우선', '14', 'gallery access', 'flat', density(40, 44, 40, 40, 32, 272, 15), (6, 8, 8), 0, 'none', 2.0, 3, 2, False,
     claude(P['access']['dark'], 'dark'), claude(P['access']['light'], 'light'), SHARED_HC),
    ('quiet-teal', '조용한 편집기 · 청록', '15', 'Claude A variant', 'flat', D_QUIET, (4, 6, 8), 0, 'none', 1.5, 2, 1, True,
     claude(P['a-teal']['dark'], 'dark'), claude(P['a-teal']['light'], 'light'), QUIET_HC),
    ('quiet-violet', '조용한 편집기 · 보라', '16', 'Claude A variant', 'flat', D_QUIET, (4, 6, 8), 0, 'none', 1.5, 2, 1, True,
     claude(P['a-violet']['dark'], 'dark'), claude(P['a-violet']['light'], 'light'), QUIET_HC),
    ('workbench-neutral', '정밀한 작업대 · 중립 회색', '17', 'Codex A variant', 'flat', D_WORKBENCH, (4, 6, 8), 0, 'none', 2.0, 2, 1, True,
     codex('dark', CV['codex-neutral']['dark']), codex('light', CV['codex-neutral']['light']), SHARED_HC),
    ('workbench-green', '정밀한 작업대 · 녹청', '18', 'Codex A variant', 'flat', D_WORKBENCH, (4, 6, 8), 0, 'none', 2.0, 2, 1, True,
     codex('dark', CV['codex-green']['dark']), codex('light', CV['codex-green']['light']), SHARED_HC),
]

# Accent presets (docs/10 §D5.3), taken from the samples that introduced them.
ACCENTS = {
    'blue': {'dark': {'action.primary.bg': '#3D6FD6', 'action.primary.hover': '#3463C4', 'accent.fg': '#8DB5FF', 'focus.ring': '#6A9BFF', 'bg.selected': '#1F2C42'},
             'light': {'action.primary.bg': '#2F66D0', 'action.primary.hover': '#2959B8', 'accent.fg': '#2459BF', 'focus.ring': '#2F6BDB', 'bg.selected': '#DFE8FA'}},
    'teal': {'dark': {'action.primary.bg': '#1A7A76', 'action.primary.hover': '#176D69', 'accent.fg': '#6FD0C9', 'focus.ring': '#6FD0C9', 'bg.selected': '#173336'},
             'light': {'action.primary.bg': '#137A76', 'action.primary.hover': '#106A67', 'accent.fg': '#0E6B67', 'focus.ring': '#137A76', 'bg.selected': '#D5EEEC'}},
    'violet': {'dark': {'action.primary.bg': '#7353D6', 'action.primary.hover': '#6747C7', 'accent.fg': '#BBA6FF', 'focus.ring': '#BBA6FF', 'bg.selected': '#2C2442'},
               'light': {'action.primary.bg': '#6342C8', 'action.primary.hover': '#5636B0', 'accent.fg': '#5535B5', 'focus.ring': '#6342C8', 'bg.selected': '#E9E2FB'}},
    'green': {'dark': {'action.primary.bg': '#2A7A63', 'action.primary.hover': '#24694F', 'accent.fg': '#8FD8BE', 'focus.ring': '#8FD8BE', 'bg.selected': '#1E3B34'},
              'light': {'action.primary.bg': '#1F6F57', 'action.primary.hover': '#195D49', 'accent.fg': '#1D6B54', 'focus.ring': '#1D6B54', 'bg.selected': '#DCEEE7'}},
    'indigo': {'dark': {'action.primary.bg': '#5468E0', 'action.primary.hover': '#4A5DCC', 'accent.fg': '#A3B0FF', 'focus.ring': '#8C9CFF', 'bg.selected': '#262D45'},
               'light': {'action.primary.bg': '#4553C9', 'action.primary.hover': '#3C49B4', 'accent.fg': '#3946B8', 'focus.ring': '#4553C9', 'bg.selected': '#E6E9FB'}},
}
for a in ACCENTS.values():
    for m in a.values():
        m['action.primary.pressed'] = shade(m['action.primary.hover'], .9)

# Hand corrections: (style, palette, token) -> (value, reason). Applied before the automatic nudges below.
FIXES = {
    ('mono', 'dark', 'action.primary.hover'): ('#D4D4D4', 'sample 09 left the blue hover of its base palette; white-on-white button needs a grey hover'),
    ('mono', 'light', 'action.primary.hover'): ('#333333', 'sample 09 left the blue hover of its base palette'),
    ('mono', 'dark', 'action.primary.pressed'): ('#BDBDBD', 'derived from the blue hover; keep the button monochrome'),
    ('mono', 'light', 'action.primary.pressed'): ('#474747', 'derived from the blue hover; keep the button monochrome'),
}
SURFACES = ['bg.canvas', 'bg.chrome', 'bg.surface', 'bg.raised', 'bg.hover', 'bg.selected']


def lum(h):
    v = int(h[1:7], 16)
    c = [((v >> s) & 255) / 255 for s in (16, 8, 0)]
    c = [x / 12.92 if x <= .04045 else ((x + .055) / 1.055) ** 2.4 for x in c]
    return .2126 * c[0] + .7152 * c[1] + .0722 * c[2]


def ratio(a, b):
    a, b = lum(a), lum(b)
    return (max(a, b) + .05) / (min(a, b) + .05)


def pairs(c):
    """(fg, bg, minimum): the same list as pod_ui::themes::contrast_pairs."""
    out = [(fg, bg, 4.5) for fg in ('text.primary', 'text.secondary', 'text.muted') for bg in SURFACES]
    out += [(fg, bg, 4.5) for fg in ('success.fg', 'warning.fg', 'danger.fg', 'info.fg') for bg in SURFACES if bg != 'bg.raised']
    out += [(t + '.fg', t + '.bg', 4.5) for t in ('success', 'warning', 'danger', 'info')]
    out += [('accent.fg', bg, 3) for bg in SURFACES]
    out += [('focus.ring', bg, 3) for bg in SURFACES[:4]]
    out += [('border.control', bg, 3) for bg in SURFACES[:4]]
    out += [('action.primary.fg', bg, 4.5) for bg in ('action.primary.bg', 'action.primary.hover', 'action.primary.pressed')]
    out += [('terminal.fg', 'terminal.bg', 4.5), ('diff.add.fg', 'diff.add.bg', 4.5), ('diff.del.fg', 'diff.del.bg', 4.5)]
    return out


def audit(name, c):
    bad = []
    for fg, bg, minimum in pairs(c):
        r = ratio(c[fg], c[bg])
        if r < minimum:
            bad.append(f'{name}: {fg} {c[fg]} on {bg} {c[bg]} = {r:.2f} < {minimum}')
    return bad


def mix(h, target, t):
    v, w = int(h[1:7], 16), int(target[1:7], 16)
    ch = [round(((v >> k) & 255) * (1 - t) + ((w >> k) & 255) * t) for k in (16, 8, 0)]
    return '#%02X%02X%02X' % tuple(ch)


def nudge(c, token, checks, toward, log, label):
    """Move `token` toward black or white in 3% steps until every (fg, bg, min) in checks passes."""
    start = c[token]
    for _ in range(40):
        if all(ratio(c[f], c[b]) >= m for f, b, m in checks):
            break
        c[token] = mix(c[token], toward, .03)
    if c[token] != start:
        log.append(f'{label}: {token} {start} -> {c[token]}')


def autofix(c, dark, log, label):
    """The automatic corrections (docs/10 §D5.2): only these three token families are ever moved."""
    light_way, dark_way = '#FFFFFF', '#000000'
    # Required control borders: away from the surfaces.
    nudge(c, 'border.control', [('border.control', b, 3) for b in SURFACES[:4]], light_way if dark else dark_way, log, label)
    # Primary buttons: darken under light text, lighten under dark text.
    fg_light = lum(c['action.primary.fg']) > .5
    for t in ('action.primary.bg', 'action.primary.hover', 'action.primary.pressed'):
        nudge(c, t, [('action.primary.fg', t, 4.5)], dark_way if fg_light else light_way, log, label)
    # Selected rows must keep every text and status colour readable.
    sel = [(f, 'bg.selected', 4.5) for f in ('text.primary', 'text.secondary', 'text.muted', 'success.fg', 'warning.fg', 'danger.fg', 'info.fg')]
    sel.append(('accent.fg', 'bg.selected', 3))
    nudge(c, 'bg.selected', sel, dark_way if dark else light_way, log, label)
    c['terminal.selection'] = c['bg.selected']


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    problems, log, index = [], [], []
    for (sid, name, sample, source, surface, dens, radius, gap, shadow, stroke, focus, border, accent_ok, dark, light, hc) in STYLES:
        colors = {'dark': dict(dark), 'light': dict(light)}
        colors.update({k: dict(v) for k, v in hc.items()})
        for (s, pal, token), (value, reason) in FIXES.items():
            if s == sid:
                log.append(f'{sid}/{pal}: {token} {colors[pal][token]} -> {value} ({reason})')
                colors[pal][token] = value
        accents = {}
        for pal, c in colors.items():
            autofix(c, pal.endswith('dark'), log, f'{sid}/{pal}')
            problems += audit(f'{sid}/{pal}', c)
            if accent_ok and not pal.startswith('hc'):
                for aid, acc in ACCENTS.items():
                    merged = {**c, **acc[pal]}
                    autofix(merged, pal.endswith('dark'), log, f'{sid}/{pal}+{aid}')
                    problems += audit(f'{sid}/{pal}+{aid}', merged)
                    accents.setdefault(aid, {})[pal] = {k: merged[k] for k in list(acc[pal]) + ['terminal.selection']}
        spec = {
            'id': sid, 'name': name, 'sample': sample, 'source': source, 'surface': surface, 'density': dens,
            'radius': {'control': radius[0], 'card': radius[1], 'overlay': radius[2]}, 'gap': gap, 'shadow': shadow,
            'icon_stroke': stroke, 'focus_width': focus, 'border_width': border, 'accent_override': accent_ok,
            'colors': colors,
        }
        if accents:
            spec['accents'] = accents
        (OUT / f'{sid}.json').write_text(json.dumps(spec, ensure_ascii=False, indent=1) + chr(10), encoding='utf-8')
        index.append(sid)
    (OUT / 'FIXES.txt').write_text('Contrast corrections applied by tools/themes/gen.py (docs/10 §D5.2)' + chr(10) + chr(10).join(log) + chr(10), encoding='utf-8')
    print(f'{len(index)} styles written to {OUT.relative_to(ROOT)}; {len(log)} corrections in FIXES.txt')
    for p in problems:
        print('CONTRAST', p)
    return 1 if problems else 0


if __name__ == '__main__':
    raise SystemExit(main())
