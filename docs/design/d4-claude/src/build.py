"""Render the D4 mockups (original Pod design, local only).

python -I build.py            -> writes ../<dir>/*.html and *.png
Rendering: locally installed Chrome via Playwright, 960x640 CSS px at DPR 1.5
(= 1440x960 physical, the same size as tools/visual/baseline at 150%).
"""
import html
import pathlib
import re
import sys

HERE = pathlib.Path(__file__).parent
OUT = HERE.parent
sys.path.insert(0, str(HERE))
from tokens import DIRECTIONS, METRICS  # noqa: E402

FAMILY = {'A': 'lucide', 'B': 'fluent', 'C': 'phosphor'}
_cache = {}


def icon(name, d='A', size=16, cls='', style=''):
    fam = FAMILY[d]
    key = (fam, name)
    if key not in _cache:
        raw = (HERE / 'icons' / fam / f'{name}.svg').read_text(encoding='utf-8')
        raw = re.sub(r'<!--.*?-->', '', raw, flags=re.S).strip()
        head, rest = raw.split('>', 1)
        head = re.sub(r'\s(class|width|height)="[^"]*"', '', head)
        raw = re.sub(r'<svg\s+', '<svg ', head, count=1) + '>' + rest
        if fam == 'lucide':
            raw = raw.replace('stroke-width="2"', 'stroke-width="1.75"')
        if fam == 'fluent':
            raw = raw.replace('<svg ', '<svg fill="currentColor" ', 1)
        _cache[key] = raw
    svg = _cache[key]
    return svg.replace('<svg ', f'<svg class="ic {cls}" width="{size}" height="{size}" style="{style}" ', 1)


def e(s):
    return html.escape(s)


# ----------------------------------------------------------------- fixture
TASKS = [
    dict(title='터미널 넓은 글자 정렬', branch='pod/wide-glyph', age='방금', open=True, sel=True, workers=[
        ('Claude · 구현', 'working', '작업 중', 'claude', '4분'),
        ('Codex · 리뷰', 'attention', '승인 필요', 'codex', '12초'),
        ('Claude · 문서', 'done', '완료', 'claude', '1분'),
        ('셸', 'idle', '대기', 'pwsh', '—'),
    ]),
    dict(title='리뷰 의견 전달 재시도', branch='pod/review-retry', age='3분', open=True, workers=[
        ('Claude · 구현', 'attention', '응답 필요', 'claude', '3분'),
        ('셸', 'done', '완료', 'pwsh', '8분'),
    ]),
    dict(title='LSP 지연 시작', branch='pod/lsp-lazy', age='9분', open=True, workers=[
        ('Codex · 구현', 'failed', '실패 종료 (1)', 'codex', '9분'),
    ]),
    dict(title='설정 화면 정리', branch='pod/settings-polish', age='21분', open=False, workers=[
        ('Codex · 구현', 'done', '완료', 'codex', '21분'), ('셸', 'done', '완료', 'pwsh', '25분')]),
    dict(title='Windows 설치 프로그램 서명', branch='pod/signing', age='1시간', open=False, workers=[
        ('Claude · 구현', 'done', '완료', 'claude', '1시간')]),
    dict(title='알림 묶음 처리', branch='pod/notify-batch', age='2시간', open=False, workers=[
        ('셸', 'idle', '대기', 'pwsh', '—')]),
]
STATUS_ICON = {'working': 'working', 'attention': 'approval', 'done': 'done', 'idle': 'idle',
               'failed': 'failed', 'question': 'question', 'unknown': 'unknown'}
RANK = ['failed', 'attention', 'working', 'idle', 'done']


def task_state(t):
    states = [w[1] for w in t['workers']]
    for s in RANK:
        if s in states:
            return s
    return 'idle'


TERM = {
    'Claude · 구현': '''<span class=dim>❯</span> 넓은 글자가 두 칸을 차지하도록 셀 너비 계산을 고칩니다.

<span class=ok>•</span> 읽기  crates/pod-ui/src/terminal.rs <span class=dim>(412줄)</span>
<span class=ok>•</span> 수정  crates/pod-ui/src/terminal.rs
    <span class=del>-  let w = glyph.advance;</span>
    <span class=add>+  let w = cell_w * width_of(ch) as f32;</span>
<span class=ok>•</span> 실행  cargo test -p pod-ui wide_glyph
    running 6 tests <span class=dim>…</span> <span class=ok>ok</span>
<span class=info>⠋</span> 셀 경계 테스트 추가 중 <span class=dim>(42초 · esc 중단)</span>''',
    'Codex · 리뷰': '''<span class=dim>codex</span>  리뷰 대상 pod/wide-glyph · 파일 3개

변경 요약
  terminal.rs  셀 폭 계산을 unicode 폭 기준으로 변경
  status.rs    스피너 폭 고정

<span class=warn>명령 실행 승인이 필요합니다</span>
  <span class=hl>$ cargo clippy -p pod-ui -- -D warnings</span>
  <span class=dim>[y]</span> 이번만 허용   <span class=dim>[a]</span> 항상 허용   <span class=dim>[n]</span> 거부''',
    'Claude · 문서': '''<span class=ok>✓</span> docs/06-terminal-view.md 갱신
  넓은 글자 표 3행 추가, 스크린샷 경로 정정
<span class=ok>✓</span> 맞춤법 검사 통과

작업 완료 <span class=dim>· 1분 12초 · 토큰 18.2k</span>
<span class=dim>❯</span> <span class=cursor> </span>''',
    '셸': '''<span class=dim>PS C:\\src\\pod&gt;</span> git status --short
 <span class=warn>M</span> crates/pod-ui/src/terminal.rs
 <span class=warn>M</span> crates/pod-ui/src/status.rs
<span class=add>??</span> crates/pod-ui/tests/wide_glyph.rs
<span class=dim>PS C:\\src\\pod&gt;</span> <span class=cursor> </span>''',
}

# ----------------------------------------------------------------- CSS
BASE_CSS = '''
*{box-sizing:border-box;margin:0;padding:0}
html,body{width:960px;height:640px;overflow:hidden}
body{background:var(--app);color:var(--text);font:400 13px/20px "Segoe UI Variable Text","Segoe UI","Malgun Gothic",sans-serif;
 -webkit-font-smoothing:antialiased;letter-spacing:0}
.mono{font-family:"Cascadia Mono",Consolas,"Malgun Gothic",monospace}
.ic{flex:none;display:block}
.tb,.top,.crumb,.sbh,.sec,.tr,.wr,.ph,.tab,.wt,.stb,.rvh,.cph,.agc,.agh,.th,.ch,.qh,.ql,.r,.btn,.seg,.kbd,.k,.pillst,.chip,.cnt{white-space:nowrap}
.row{display:flex;align-items:center}
.col{display:flex;flex-direction:column}
.grow{flex:1;min-width:0}
.ell{white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.t2{color:var(--text2)} .mu{color:var(--muted)} .acc{color:var(--accent_fg)}
.s-working{color:var(--working)} .s-attention{color:var(--attention)} .s-done{color:var(--done)}
.s-failed{color:var(--failed)} .s-idle{color:var(--idle)}
.dot{width:8px;height:8px;border-radius:50%;flex:none}
.d-working{background:var(--working)} .d-attention{background:var(--attention)} .d-done{background:var(--done)}
.d-failed{background:var(--failed)} .d-idle{background:transparent;box-shadow:inset 0 0 0 1.5px var(--idle)}
pre{font:12px/18px "Cascadia Mono",Consolas,"Malgun Gothic",monospace;color:var(--term_fg);white-space:pre}
pre .dim{color:var(--muted)} pre .ok{color:var(--done)} pre .warn{color:var(--attention)} pre .info{color:var(--working)}
pre .add{color:var(--add_fg)} pre .del{color:var(--del_fg)} pre .hl{color:var(--text);font-weight:600}
pre .cursor{background:var(--text);display:inline-block;width:7px;height:15px;vertical-align:-3px}
.kbd{font:11px/16px "Segoe UI","Malgun Gothic";color:var(--muted);border:1px solid var(--hairline);border-radius:3px;padding:0 4px}
.caption{display:flex;height:100%}
.caption div{width:46px;display:flex;align-items:center;justify-content:center;color:var(--text2)}
.tip{position:absolute;background:var(--overlay);color:var(--text);border:1px solid var(--strong);border-radius:4px;padding:2px 8px;font-size:12px;line-height:18px;
 box-shadow:0 4px 12px rgba(0,0,0,.25);white-space:nowrap;z-index:9}
'''


def vars_css(d, theme):
    t = DIRECTIONS[d][theme]
    return ':root{' + ';'.join(f'--{k}:{v}' for k, v in t.items()) + '}'


def page(d, theme, body, extra_css=''):
    dark = 'dark' in theme
    shadow = 'rgba(0,0,0,.45)' if dark else 'rgba(16,24,40,.14)'
    return f'''<!doctype html><html lang="ko"><head><meta charset="utf-8"><style>{vars_css(d, theme)}
:root{{--shadow:{shadow}}}{BASE_CSS}{extra_css}</style></head><body class="{d} {theme}">{body}</body></html>'''


# ================================================================ Direction A
A_CSS = '''
.tb{height:36px;display:flex;align-items:center;background:var(--app);border-bottom:1px solid var(--hairline);padding-left:12px;gap:8px}
.logo{width:18px;height:18px;border-radius:5px;background:var(--accent);color:#fff;font:700 11px/18px "Segoe UI";text-align:center}
.crumb{display:flex;align-items:center;gap:6px;color:var(--text2);font-size:13px}
.crumb b{color:var(--text);font-weight:600}
.pill{margin:0 auto;width:340px;height:26px;border-radius:6px;background:var(--panel);border:1px solid var(--hairline);display:flex;align-items:center;gap:8px;padding:0 6px 0 10px;color:var(--muted)}
.ib{width:28px;height:28px;border-radius:4px;display:flex;align-items:center;justify-content:center;color:var(--text2);position:relative}
.ib.on{background:var(--active);color:var(--text)}
.ib .badge{position:absolute;top:4px;right:4px;width:7px;height:7px;border-radius:50%;background:var(--attention);box-shadow:0 0 0 2px var(--app)}
.main{display:flex;height:580px}
.sb{width:248px;background:var(--sidebar);border-right:1px solid var(--hairline);display:flex;flex-direction:column}
.sbh{height:36px;display:flex;align-items:center;padding:0 6px 0 12px;gap:2px}
.sbh .ib{width:26px;height:26px}
.sec{height:26px;display:flex;align-items:center;gap:6px;padding:0 8px 0 10px;font-size:12px;color:var(--muted);font-weight:600}
.tr{height:28px;margin:0 6px;border-radius:4px;display:flex;align-items:center;gap:8px;padding:0 8px 0 4px}
.tr .chev{color:var(--muted);width:14px}
.tr .ttl{flex:1;min-width:0;font-weight:500}
.tr .meta{color:var(--muted);font-size:12px}
.tr.cur{background:var(--selected_quiet)}
.wr{height:28px;margin:0 6px;border-radius:4px;display:flex;align-items:center;gap:8px;padding:0 8px 0 30px;color:var(--text2)}
.wr.sel{background:var(--selected);color:var(--text)}
.wr.hov{background:var(--hover)}
.wr .st{font-size:12px}
.cnt{font-size:11px;line-height:16px;padding:0 5px;border-radius:8px;font-weight:600}
.cnt.attention{background:var(--attention_bg);color:var(--attention)}
.cnt.failed{background:var(--failed_bg);color:var(--failed)}
.content{flex:1;min-width:0;display:flex;flex-direction:column;background:var(--panel);overflow:hidden}
.tabs{height:32px;display:flex;align-items:stretch;overflow:hidden;background:var(--app);border-bottom:1px solid var(--hairline)}
.tab{display:flex;align-items:center;gap:6px;padding:0 10px 0 12px;color:var(--text2);border-right:1px solid var(--hairline);font-size:13px;position:relative}
.tab.on{background:var(--panel);color:var(--text);margin-bottom:-1px}
.tab.on:before{content:"";position:absolute;left:0;right:0;top:0;height:2px;background:var(--accent)}
.tab .x{color:var(--muted);opacity:0}
.tab.on .x{opacity:1}
.grid{flex:1;display:grid;grid-template-columns:1fr 1fr;grid-template-rows:1fr 1fr;gap:1px;background:var(--hairline);min-height:0}
.pane{background:var(--term_bg);display:flex;flex-direction:column;min-width:0;min-height:0;position:relative}
.ph{height:30px;display:flex;align-items:center;gap:8px;padding:0 6px 0 10px;border-bottom:1px solid var(--hairline);color:var(--text2);font-size:12px}
.ph .nm{color:var(--text);font-size:13px;font-weight:500}
.pane.focus{box-shadow:inset 0 0 0 1px var(--focus)}
.pane pre{padding:8px 12px;overflow:hidden;flex:1}
.ph .tools{display:flex;margin-left:auto;gap:0}
.ph .tools .ib{width:24px;height:24px}
.stb{height:24px;display:flex;align-items:center;background:var(--app);border-top:1px solid var(--hairline);padding:0 6px;font-size:12px;color:var(--text2);gap:0}
.stb .g{display:flex;align-items:center;gap:6px;padding:0 8px;height:100%}
.stb .sep{width:1px;height:12px;background:var(--strong)}
.stb .g b{font-weight:600;color:var(--text)}
.btn{height:28px;padding:0 12px;border-radius:4px;display:inline-flex;align-items:center;gap:6px;font-size:13px;font-weight:500;color:var(--text);background:transparent;border:1px solid var(--strong)}
.btn.pri{background:var(--accent);color:var(--on_accent);border-color:var(--accent)}
.btn.ghost{border-color:transparent;color:var(--text2)}
.btn.sm{height:24px;padding:0 8px;font-size:12px}
.seg{display:inline-flex;height:26px;border-radius:5px;background:var(--app);border:1px solid var(--hairline);padding:2px;gap:2px}
.seg div{display:flex;align-items:center;gap:5px;padding:0 8px;border-radius:3px;color:var(--text2);font-size:12px}
.seg div.on{background:var(--raised);color:var(--text);box-shadow:0 0 0 1px var(--strong)}
.banner{display:flex;align-items:flex-start;gap:8px;padding:8px 10px;border-radius:6px;font-size:12px;line-height:18px}
.banner.info{background:var(--working_bg);color:var(--text)} .banner.info .ic{color:var(--working)}
.banner.err{background:var(--failed_bg);color:var(--text)} .banner.err .ic{color:var(--failed)}
.banner.warn{background:var(--attention_bg);color:var(--text)} .banner.warn .ic{color:var(--attention)}
.banner .act{margin-left:auto;font-weight:600;color:var(--accent_fg);white-space:nowrap}
'''


def a_titlebar(d, crumb, bell_dot=True):
    I = lambda n, s=16: icon(n, d, s)
    return f'''<div class="tb"><div class="logo">P</div>
<div class="crumb"><span>pod</span>{icon('chevron-right', d, 12)}<b>{e(crumb)}</b></div>
<div class="pill">{I('search', 14)}<span class="grow">작업, 터미널, 명령 검색</span><span class="kbd">Ctrl K</span></div>
<div class="ib">{I('panel-left')}</div><div class="ib">{I('panel-right')}</div>
<div class="ib">{I('bell')}{'<span class=badge></span>' if bell_dot else ''}</div><div class="ib">{I('settings')}</div>
<div class="caption"><div>{I('minus')}</div><div>{I('square', 14)}</div><div>{I('x')}</div></div></div>'''


def a_sidebar(d, hover_worker=None, sel_worker=('터미널 넓은 글자 정렬', 'Codex · 리뷰')):
    I = lambda n, s=16, c='': icon(n, d, s, c)
    out = [f'''<div class="sb"><div class="sbh"><span style="font-weight:600">작업</span><span class="mu" style="margin-left:6px;font-size:12px">6</span>
<span class="grow"></span><div class="ib">{I('filter')}</div><div class="ib">{I('plus')}</div><div class="ib">{I('ellipsis')}</div></div>
<div class="sec">{I('chevron-down', 12)}{I('folder-git', 14)}<span>pod</span><span class="grow"></span><span style="font-weight:400">C:\\src\\pod</span></div>''']
    for t in TASKS:
        st = task_state(t)
        need = sum(1 for w in t['workers'] if w[1] in ('attention', 'failed'))
        meta = (f'<span class="cnt {st}">{need}</span>' if need else f'<span class="meta">{t["age"]}</span>')
        cur = ' cur' if t.get('sel') else ''
        out.append(f'''<div class="tr{cur}"><span class="chev">{I('chevron-down' if t['open'] else 'chevron-right', 14)}</span>
<span class="dot d-{st}"></span><span class="ttl ell">{e(t['title'])}</span>{meta}</div>''')
        if t['open']:
            for name, s, label, prog, age in t['workers']:
                sel = (t['title'], name) == sel_worker
                hov = (t['title'], name) == hover_worker
                cls = ' sel' if sel else (' hov' if hov else '')
                tail = f'<span class="st s-{s}">{e(label)}</span>' if (sel or hov or s == 'failed') else ''
                out.append(f'''<div class="wr{cls}">{I(STATUS_ICON[s], 14, 's-' + s)}<span class="grow ell">{e(name)}</span>{tail}</div>''')
    out.append('''<div class="sec" style="margin-top:6px">''' + I('chevron-right', 12) + I('folder-git', 14) +
               '<span>docs-site</span><span class="grow"></span><span style="font-weight:400">작업 1</span></div></div>')
    return ''.join(out)


def a_statusbar(d):
    I = lambda n, s=14, c='': icon(n, d, s, c)
    return f'''<div class="stb"><div class="g">{I('plug', 14, 's-done')}<span>podd</span></div><div class="sep"></div>
<div class="g">{I('git-branch')}<span>pod/wide-glyph</span></div><div class="grow"></div>
<div class="g" title="작업 상태">{I('working', 14, 's-working')}<b>3</b>{I('approval', 14, 's-attention')}<b>2</b>{I('failed', 14, 's-failed')}<b>1</b>{I('done', 14, 's-done')}<b>5</b></div>
<div class="sep"></div><div class="g">{I('terminal')}<span>터미널 12</span></div></div>'''


def a_tabs(d, active):
    I = lambda n, s=14: icon(n, d, s)
    tabs = [('terminal', '터미널', '4'), ('file-diff', '리뷰', ''), ('file-diff', 'terminal.rs', '')]
    out = ['<div class="tabs">']
    for i, (ic, label, n) in enumerate(tabs):
        on = ' on' if i == active else ''
        extra = f'<span class="mu" style="font-size:12px">{n}</span>' if n else ''
        mod = '<span class="dot" style="width:6px;height:6px;background:var(--text2)"></span>' if label.endswith('.rs') else ''
        out.append(f'<div class="tab{on}">{I(ic)}<span>{label}</span>{extra}{mod}<span class="x">{I("x", 12)}</span></div>')
    out.append(f'<div class="grow"></div><div class="row" style="padding:0 6px;gap:2px"><div class="ib on">{I("grid")}</div><div class="ib">{I("split")}</div><div class="ib">{I("plus")}</div></div></div>')
    return ''.join(out)


def a_pane(d, name, state, label, branch, focus=False, tools=False):
    I = lambda n, s=14, c='': icon(n, d, s, c)
    t = f'<div class="tools"><div class="ib">{I("maximize")}</div><div class="ib">{I("ellipsis")}</div></div>' if tools else f'<span class="grow"></span>'
    return f'''<div class="pane{' focus' if focus else ''}"><div class="ph">{I(STATUS_ICON[state], 14, 's-' + state)}<span class="nm">{e(name)}</span>
<span class="s-{state}">{e(label)}</span><span class="mu">· {branch}</span>{t}</div><pre>{TERM[name]}</pre></div>'''


def a_workspace(theme):
    d = 'A'
    t = TASKS[0]
    panes = ''.join(a_pane(d, n, s, l, f'{p} · {a}', focus=(n == 'Codex · 리뷰'), tools=(n == 'Codex · 리뷰')) for n, s, l, p, a in t['workers'])
    body = f'''{a_titlebar(d, t['title'])}<div class="main">{a_sidebar(d)}
<div class="content">{a_tabs(d, 0)}<div class="grid">{panes}</div></div></div>{a_statusbar(d)}'''
    return page(d, theme, body, A_CSS)


DIFF = [
    ('hunk', '', '', '@@ -212,9 +212,14 @@ fn layout_line(&mut self, row: &Row)'),
    ('ctx', '212', '212', '    for (col, cell) in row.cells().enumerate() {'),
    ('ctx', '213', '213', '        let ch = cell.ch;'),
    ('del', '214', '', '        let w = glyph.advance;'),
    ('add', '', '214', '        // 넓은 글자는 두 칸을 차지한다'),
    ('add', '', '215', '        let cols = width_of(ch).max(1);'),
    ('add', '', '216', '        let w = self.cell_w * cols as f32;'),
    ('comment', '', '', ''),
    ('ctx', '215', '217', '        let x = self.origin.x + col as f32 * self.cell_w;'),
    ('ctx', '216', '218', '        self.quads.push(Quad::glyph(x, w, ch));'),
    ('add', '', '219', '        if cols == 2 { self.skip_next = true; }'),
    ('ctx', '217', '220', '    }'),
    ('hunk', '', '', '@@ -301,6 +306,9 @@ fn width_of(ch: char) -> usize'),
    ('ctx', '301', '306', 'fn width_of(ch: char) -> usize {'),
    ('add', '', '307', '    if let Some(w) = WIDTH_CACHE.get(ch) { return w; }'),
    ('ctx', '302', '308', '    UnicodeWidthChar::width(ch).unwrap_or(1)'),
]


def a_review(theme):
    d = 'A'
    I = lambda n, s=16, c='': icon(n, d, s, c)
    files = [('terminal.rs', 'crates/pod-ui/src', 'M', '+24', '−6', True), ('status.rs', 'crates/pod-ui/src', 'M', '+3', '−1', False),
             ('wide_glyph.rs', 'crates/pod-ui/tests', 'A', '+58', '', False)]
    fl = ''.join(f'''<div class="wr{' sel' if s else ''}" style="padding-left:10px;height:28px">{I('file-diff', 14)}<span class="grow ell"><span style="color:var(--text)">{n}</span> <span class="mu" style="font-size:12px">{p}</span></span>
<span style="font-size:11px;font-weight:600" class="{'s-done' if k == 'A' else 's-attention'}">{k}</span></div>
<div style="margin:-6px 6px 4px 34px;font-size:11px;line-height:14px" class="mono"><span style="color:var(--add_fg)">{a}</span> <span style="color:var(--del_fg)">{r}</span></div>''' for n, p, k, a, r, s in files)
    rows = []
    for kind, o, nn, text in DIFF:
        if kind == 'hunk':
            rows.append(f'<div class="dl hunk"><span class="ln"></span><span class="ln"></span><span class="code">{e(text)}</span></div>')
        elif kind == 'comment':
            rows.append(f'''<div class="thread"><div class="row" style="gap:8px"><span class="av">나</span><b style="font-weight:600">나</b><span class="mu">2분 전 · 216행</span>
<span class="grow"></span><span class="cnt attention">미해결</span></div><div style="margin:4px 0 6px 28px">넓은 글자 폭은 매 프레임 계산하지 말고 캐시해 주세요.</div>
<div class="row" style="gap:8px;margin-left:28px">{I('bot', 14, 'mu')}<span class="t2">Codex · 리뷰</span><span class="mu">· 다음 수정에 반영 예정</span></div></div>''')
        else:
            sign = {'add': '+', 'del': '−', 'ctx': ' '}[kind]
            rows.append(f'<div class="dl {kind}"><span class="ln">{o}</span><span class="ln">{nn}</span><span class="code"><i>{sign}</i>{e(text)}</span></div>')
    css = A_CSS + '''
.rvh{height:36px;display:flex;align-items:center;gap:8px;padding:0 8px 0 12px;border-bottom:1px solid var(--hairline);background:var(--panel)}
.rvh .vs{width:1px;height:16px;background:var(--hairline);margin:0 2px}
.rvh .ib{width:26px;height:26px}
.rvb{flex:1;display:flex;min-height:0}
.fl{width:220px;border-right:1px solid var(--hairline);padding-top:6px;background:var(--panel)}
.diff{flex:1;min-width:0;overflow:hidden;font:12px/20px "Cascadia Mono",Consolas,"Malgun Gothic",monospace;padding-top:4px}
.dl{display:flex;height:20px;white-space:pre}
.dl .ln{width:40px;text-align:right;padding-right:8px;color:var(--muted);flex:none;font-size:11px}
.dl .code{flex:1;padding-left:6px;color:var(--term_fg)} .dl .code i{font-style:normal;color:var(--muted);display:inline-block;width:14px}
.dl.add{background:var(--add_bg)} .dl.add .code i{color:var(--add_fg)}
.dl.del{background:var(--del_bg)} .dl.del .code i{color:var(--del_fg)}
.dl.hunk{color:var(--muted);background:var(--app)} .dl.hunk .code{color:var(--muted)}
.thread{margin:4px 16px 6px 92px;max-width:420px;padding:8px 10px;border:1px solid var(--strong);border-radius:6px;background:var(--raised);font:13px/20px "Segoe UI Variable Text","Segoe UI","Malgun Gothic";white-space:normal;
 box-shadow:inset 3px 0 0 var(--attention)}
.av{width:20px;height:20px;border-radius:50%;background:var(--selected);color:var(--accent_fg);font-size:11px;display:flex;align-items:center;justify-content:center;font-weight:600}
.cp{width:280px;border-left:1px solid var(--hairline);display:flex;flex-direction:column;background:var(--panel)}
.cph{height:32px;display:flex;align-items:center;padding:0 8px 0 12px;gap:6px;font-weight:600}
.card{margin:0 10px 8px;padding:8px 10px;border-radius:6px;border:1px solid var(--hairline);font-size:12px;line-height:18px}
.card.open{border-color:var(--strong)}
.composer{margin:0 10px;border:1px solid var(--control);border-radius:6px;padding:8px 10px;height:64px;color:var(--muted);font-size:12px;background:var(--app)}
'''
    body = f'''{a_titlebar(d, TASKS[0]['title'])}<div class="main">
<div class="content">{a_tabs(d, 1)}
<div class="rvh">{I('git-branch', 14, 'mu')}<span style="font-weight:600">pod/wide-glyph</span><span class="mu">→ main</span><span class="mu" style="font-size:12px">· 파일 3 ·</span>
<span class="mono" style="font-size:12px;color:var(--add_fg)">+85</span><span class="mono" style="font-size:12px;color:var(--del_fg)">−7</span><span class="grow"></span>
<div class="seg"><div>{I('split', 14)}나란히</div><div class="on">{I('unified', 14)}한 줄로</div></div><div class="vs"></div>
<div class="ib on" title="줄바꿈">{I('wrap')}</div><div class="ib">{I('expand')}</div><div class="vs"></div>
<div class="ib">{I('arrow-up')}</div><div class="ib">{I('arrow-down')}</div><div class="ib">{I('chevron-left')}</div><div class="ib">{I('chevron-right')}</div><div class="vs"></div>
<div class="ib">{I('refresh')}</div><div class="ib">{I('ellipsis')}</div></div>
<div class="rvb"><div class="fl"><div class="sec">변경된 파일</div>{fl}</div><div class="diff">{''.join(rows)}</div>
<div class="cp"><div class="cph">{I('comment', 14, 'mu')}의견<span class="mu" style="font-weight:400;font-size:12px">미해결 1 · 전체 2</span><span class="grow"></span><div class="ib">{I('filter', 14)}</div></div>
<div class="card open"><div class="row" style="gap:6px"><span class="dot d-attention"></span><b style="font-weight:600">terminal.rs:216</b><span class="grow"></span><span class="mu">2분</span></div><div class="t2" style="margin-top:2px">넓은 글자 폭은 매 프레임 계산하지 말고 캐시해 주세요.</div></div>
<div class="card"><div class="row" style="gap:6px">{I('check', 12, 's-done')}<b style="font-weight:600" class="t2">status.rs:88</b><span class="grow"></span><span class="mu">해결됨</span></div><div class="mu" style="margin-top:2px">스피너 폭을 고정했는지 확인</div></div>
<div class="grow"></div>
<div class="banner info" style="margin:0 10px 8px">{I('working', 14)}<span>Claude · 구현이 작업 중입니다. 대기 상태가 되면 전달합니다.</span></div>
<div class="composer">에이전트에게 보낼 요청 (선택, 의견보다 먼저 전달)</div>
<div class="row" style="gap:8px;padding:8px 10px 10px"><span class="mu" style="font-size:12px">받는 에이전트: Claude · 구현</span><span class="grow"></span><span class="btn pri sm">{I('send', 14)}미해결 1개 보내기</span></div>
</div></div></div></div>{a_statusbar(d)}'''
    return page(d, theme, body, css)


def a_agents(theme):
    d = 'A'
    I = lambda n, s=16, c='': icon(n, d, s, c)
    css = A_CSS + '''
.ag{width:430px;border-left:1px solid var(--hairline);background:var(--panel);display:flex;flex-direction:column}
.agh{height:32px;display:flex;align-items:center;gap:6px;padding:0 6px 0 12px;font-weight:600;border-bottom:1px solid var(--hairline)}
.agc{display:grid;grid-template-columns:1fr 64px 92px 40px;align-items:center;column-gap:8px;padding:0 12px 0 8px;height:28px}
.agc.head{height:26px;font-size:11px;color:var(--muted);font-weight:600;border-bottom:1px solid var(--hairline)}
.agc.task{font-weight:500}
.agc.sel{background:var(--selected)}
.agc .nm{display:flex;align-items:center;gap:6px;min-width:0}
.agc .pr{font-size:12px;color:var(--muted)} .agc .tm{font-size:12px;color:var(--muted);text-align:right}
.agc .stc{display:flex;align-items:center;gap:5px;font-size:12px}
'''
    rows = ['<div class="agc head"><span style="padding-left:20px">이름</span><span>프로그램</span><span>상태</span><span style="text-align:right">경과</span></div>']
    for t in TASKS[:4]:
        st = task_state(t)
        labels = {'failed': '실패 있음', 'attention': '확인 필요', 'working': '작업 중', 'done': '완료', 'idle': '대기'}
        rows.append(f'''<div class="agc task"><span class="nm">{I('chevron-down' if t['open'] else 'chevron-right', 14, 'mu')}{I('layers', 14, 'mu')}<span class="ell">{e(t['title'])}</span></span>
<span class="pr">작업 {len(t['workers'])}</span><span class="stc s-{st}"><span class="dot d-{st}"></span>{labels[st]}</span><span class="tm">{t['age']}</span></div>''')
        if t['open']:
            for name, s, label, prog, age in t['workers']:
                sel = ' sel' if (t['title'], name) == (TASKS[0]['title'], 'Codex · 리뷰') else ''
                rows.append(f'''<div class="agc{sel}"><span class="nm" style="padding-left:34px">{I('terminal', 14, 'mu')}<span class="ell">{e(name)}</span></span>
<span class="pr mono" style="font-size:11px">{prog}</span><span class="stc s-{s}">{I(STATUS_ICON[s], 14)}{e(label)}</span><span class="tm">{age}</span></div>''')
    t = TASKS[0]
    panes = ''.join(a_pane(d, n, s, l, f'{p} · {a}', focus=(n == 'Codex · 리뷰')) for n, s, l, p, a in t['workers'][:2])
    body = f'''{a_titlebar(d, '에이전트')}<div class="main">{a_sidebar(d)}
<div class="content">{a_tabs(d, 0)}<div class="grid" style="grid-template-columns:1fr">{panes}</div></div>
<div class="ag"><div class="agh">{I('bot', 16, 'mu')}에이전트<span class="mu" style="font-weight:400;font-size:12px">실행 7 · 확인 필요 2</span><span class="grow"></span><div class="ib">{I('filter')}</div><div class="ib">{I('x')}</div></div>
<div class="banner err" style="margin:8px 10px">{I('alert', 14)}<span>pod/lsp-lazy · Codex · 구현이 코드 1로 종료되었습니다.</span><span class="act">로그 보기</span></div>
{''.join(rows)}</div></div>{a_statusbar(d)}'''
    return page(d, theme, body, css)


def a_components(theme):
    d = 'A'
    I = lambda n, s=16, c='': icon(n, d, s, c)
    css = A_CSS + '''
body{padding:20px;display:grid;grid-template-columns:1fr 1fr;gap:20px;background:var(--app)}
.blk{background:var(--panel);border:1px solid var(--hairline);border-radius:8px;padding:14px;display:flex;flex-direction:column;gap:10px}
.lbl{font-size:11px;color:var(--muted);font-weight:600}
.dlg{background:var(--overlay);border:1px solid var(--strong);border-radius:8px;box-shadow:0 12px 32px var(--shadow);padding:16px;display:flex;flex-direction:column;gap:12px}
.field{height:30px;border:1px solid var(--control);border-radius:4px;padding:0 10px;display:flex;align-items:center;background:var(--panel)}
.field.f{border-color:var(--focus);box-shadow:0 0 0 1px var(--focus)}
.toast{background:var(--overlay);border:1px solid var(--strong);border-radius:8px;box-shadow:0 4px 12px var(--shadow);padding:10px 12px;display:flex;gap:10px;align-items:flex-start}
.empty{align-items:center;text-align:center;padding:22px}
'''
    body = f'''<div class="col" style="gap:20px">
<div class="dlg"><div class="row"><b style="font-size:15px;line-height:22px;font-weight:600">새 터미널</b><span class="grow"></span>{I('x', 16, 'mu')}</div>
<div class="col" style="gap:6px"><span class="lbl">프로그램</span><div class="seg" style="height:30px;align-self:flex-start"><div class="on">{I('terminal', 14)}셸</div><div>{I('bot', 14)}Claude</div><div>{I('bot', 14)}Codex</div><div>사용자 지정</div></div></div>
<div class="col" style="gap:6px"><span class="lbl">작업 폴더</span><div class="field mono" style="font-size:12px">C:\\src\\pod</div></div>
<div class="col" style="gap:6px"><span class="lbl">이름 (선택)</span><div class="field f mu">예: review-bot</div></div>
<div class="row" style="gap:8px;justify-content:flex-end"><span class="btn">취소</span><span class="btn pri">실행 <span class="kbd" style="color:inherit;border-color:rgba(255,255,255,.4)">Enter</span></span></div></div>
<div class="blk"><span class="lbl">배너 · 인라인 (아이콘 + 문장 + 동작)</span>
<div class="banner err">{I('alert', 14)}<span>변경 사항을 읽지 못했습니다. 작업 폴더가 없습니다.</span><span class="act">다시 시도</span></div>
<div class="banner warn">{I('approval', 14)}<span>Codex · 리뷰가 명령 실행 승인을 기다립니다.</span><span class="act">열기</span></div>
<div class="banner info">{I('plug', 14)}<span>podd에 다시 연결하는 중입니다… 터미널은 계속 실행됩니다.</span></div></div>
</div>
<div class="col" style="gap:20px">
<div class="blk"><span class="lbl">버튼 · 아이콘 버튼 (툴팁) · 분할 제어</span>
<div class="row" style="gap:8px"><span class="btn pri">{I('plus', 14)}새 작업</span><span class="btn">저장소 열기</span><span class="btn ghost">취소</span></div>
<div class="row" style="gap:4px;position:relative;height:56px;align-items:flex-start"><div class="ib">{I('plus')}</div><div class="ib on">{I('grid')}</div><div class="ib">{I('split')}</div><div class="ib">{I('refresh')}</div>
<div class="tip" style="left:20px;top:32px">격자 보기 <span class="kbd">Ctrl Alt G</span></div></div>
<div class="seg" style="align-self:flex-start"><div class="on">{I("split", 14)}나란히</div><div>{I("unified", 14)}한 줄로</div></div></div>
<div class="toast">{I('done', 16, 's-done')}<div class="grow"><b style="font-weight:600">Claude · 문서 완료</b><div class="t2" style="font-size:12px;line-height:18px">터미널 넓은 글자 정렬 · 1분 12초</div></div><span class="act acc" style="font-weight:600;font-size:12px">열기</span>{I('x', 14, 'mu')}</div>
<div class="blk empty"><div style="width:40px;height:40px;border-radius:8px;background:var(--app);display:flex;align-items:center;justify-content:center;color:var(--text2)">{I('layers', 20)}</div>
<b style="font-size:15px;line-height:22px;font-weight:600">이 저장소에 아직 작업이 없습니다</b>
<span class="t2" style="max-width:300px">작업을 만들면 전용 브랜치와 작업 폴더가 준비되고, 그 안에서 에이전트 터미널을 시작합니다.</span>
<div class="row" style="gap:8px"><span class="btn pri">{I('plus', 14)}새 작업</span><span class="btn ghost">터미널만 열기 <span class="kbd">Ctrl Shift T</span></span></div></div>
</div>'''
    return page(d, theme, body, css)


# ================================================================ Direction B
B_CSS = '''
body{background:var(--app)}
.top{height:40px;display:flex;align-items:center;gap:6px;padding-left:10px}
.logo{width:20px;height:20px;border-radius:6px;background:linear-gradient(135deg,var(--accent),var(--accent_fg));color:#fff;font:700 11px/20px "Segoe UI";text-align:center}
.wtabs{display:flex;gap:4px;margin-left:8px;align-self:flex-end;height:32px}
.wt{display:flex;align-items:center;gap:6px;padding:0 12px;border-radius:8px 8px 0 0;color:var(--text2)}
.wt.on{background:var(--panel);color:var(--text)}
.srch{margin-left:auto;width:260px;height:28px;border-radius:8px;background:var(--hover);display:flex;align-items:center;gap:8px;padding:0 10px;color:var(--muted)}
.ib{width:30px;height:30px;border-radius:6px;display:flex;align-items:center;justify-content:center;color:var(--text2)}
.ib.on{background:var(--panel);color:var(--text);box-shadow:0 1px 2px var(--shadow)}
.caption{display:flex;height:40px}.caption div{width:46px;display:flex;align-items:center;justify-content:center;color:var(--text2)}
.main{display:flex;height:600px;padding:0 6px 6px 0}
.sb{width:256px;display:flex;flex-direction:column;padding:4px 8px}
.sbh{height:34px;display:flex;align-items:center;gap:4px;padding-left:6px}
.new{height:32px;border-radius:8px;background:var(--accent);color:var(--on_accent);display:flex;align-items:center;gap:8px;padding:0 10px;font-weight:600;margin:4px 0 10px}
.sec{height:26px;display:flex;align-items:center;gap:6px;padding:0 6px;font-size:12px;color:var(--muted);font-weight:600}
.tr{height:30px;border-radius:8px;display:flex;align-items:center;gap:8px;padding:0 8px}
.tr .ttl{flex:1;min-width:0;font-weight:500}
.tr.cur{background:var(--panel);box-shadow:0 1px 2px var(--shadow)}
.wr{height:28px;border-radius:6px;display:flex;align-items:center;gap:8px;padding:0 8px 0 28px;color:var(--text2)}
.wr.sel{background:var(--selected);color:var(--text)}
.light .wr.sel{box-shadow:0 1px 2px var(--shadow);background:var(--panel)}
.meta{font-size:12px;color:var(--muted)}
.chip{font-size:11px;line-height:18px;padding:0 7px;border-radius:9px;font-weight:600}
.chip.attention{background:var(--attention_bg);color:var(--attention)} .chip.failed{background:var(--failed_bg);color:var(--failed)}
.work{flex:1;min-width:0;display:grid;grid-template-columns:1fr 1fr;grid-template-rows:1fr 1fr;gap:6px}
.card{background:var(--panel);border-radius:10px;display:flex;flex-direction:column;min-height:0;min-width:0;box-shadow:0 1px 3px var(--shadow)}
.dark .card{box-shadow:none}
.card.focus{box-shadow:0 0 0 2px var(--focus)}
.ch{height:34px;display:flex;align-items:center;gap:8px;padding:0 8px 0 12px;font-size:12px;color:var(--text2)}
.ch .nm{font-size:13px;color:var(--text);font-weight:600}
.card pre{padding:4px 14px 10px;overflow:hidden;flex:1}
.foot{position:absolute;left:262px;right:6px;bottom:0;height:0}
.stb{position:absolute;right:16px;bottom:14px;display:flex;gap:4px;padding:3px;border-radius:10px;background:var(--overlay);box-shadow:0 6px 20px var(--shadow);font-size:12px;color:var(--text2)}
.stb .g{display:flex;align-items:center;gap:5px;padding:2px 8px;border-radius:7px}
.stb b{color:var(--text);font-weight:600}
'''


def b_workspace(theme):
    d = 'B'
    I = lambda n, s=16, c='': icon(n, d, s, c)
    side = [f'''<div class="sb"><div class="new">{I('plus')}새 작업<span class="grow"></span><span style="opacity:.75;font-weight:400;font-size:12px">Ctrl N</span></div>
<div class="sec">{I('folder-git', 14)}pod<span class="grow"></span>{I('filter', 14)}</div>''']
    for t in TASKS:
        st = task_state(t)
        need = sum(1 for w in t['workers'] if w[1] in ('attention', 'failed'))
        meta = f'<span class="chip {st}">{need}</span>' if need else f'<span class="meta">{t["age"]}</span>'
        side.append(f'<div class="tr{" cur" if t.get("sel") else ""}"><span class="dot d-{st}"></span><span class="ttl ell">{e(t["title"])}</span>{meta}</div>')
        if t['open'] and t.get('sel'):
            for name, s, label, _, _ in t['workers']:
                sel = name == 'Codex · 리뷰'
                tail = f'<span class="meta s-{s}">{e(label)}</span>' if sel else ''
                side.append(f'<div class="wr{" sel" if sel else ""}">{I(STATUS_ICON[s], 16, "s-" + s)}<span class="grow ell">{e(name)}</span>{tail}</div>')
    side.append(f'<div class="sec" style="margin-top:8px">{I("folder-git", 14)}docs-site</div></div>')
    t = TASKS[0]
    cards = ''
    for n, s, l, _, _ in t['workers']:
        f = n == 'Codex · 리뷰'
        cards += f'''<div class="card{' focus' if f else ''}"><div class="ch">{I(STATUS_ICON[s], 16, 's-' + s)}<span class="nm">{e(n)}</span><span class="s-{s}">{e(l)}</span>
<span class="grow"></span>{I('maximize', 16, 'mu') if f else ''}{I('ellipsis', 16, 'mu')}</div><pre>{TERM[n]}</pre></div>'''
    body = f'''<div class="top"><div class="logo">P</div><span style="font-weight:600">pod</span>
<div class="wtabs"><div class="wt on">{I('terminal')}터미널 넓은 글자 정렬</div><div class="wt">{I('file-diff')}리뷰</div><div class="wt">{I('bot')}에이전트</div></div>
<div class="srch">{I('search', 14)}<span class="grow">검색 및 명령</span><span style="font-size:11px">Ctrl K</span></div>
<div class="ib">{I('bell')}</div><div class="ib">{I('settings')}</div><div class="caption"><div>{I('minus')}</div><div>{I('square', 14)}</div><div>{I('x')}</div></div></div>
<div class="main">{''.join(side)}<div class="work">{cards}</div></div>
<div class="stb"><div class="g">{I('plug', 14, 's-done')}podd</div><div class="g">{I('working', 14, 's-working')}<b>3</b></div><div class="g">{I('approval', 14, 's-attention')}<b>2</b></div><div class="g">{I('failed', 14, 's-failed')}<b>1</b></div><div class="g">{I('done', 14, 's-done')}<b>5</b></div></div>'''
    return page(d, theme, body, B_CSS)


# ================================================================ Direction C
C_CSS = '''
.tb{height:36px;display:flex;align-items:center;gap:10px;padding-left:12px;background:var(--app);border-bottom:1px solid var(--hairline)}
.logo{width:18px;height:18px;border-radius:4px;background:var(--accent);color:#fff;font:700 11px/18px "Segoe UI";text-align:center}
.sum{display:flex;gap:6px;margin-left:12px}
.sum .k{display:flex;align-items:center;gap:6px;height:24px;padding:0 10px;border-radius:12px;font-size:12px;font-weight:600}
.k.working{background:var(--working_bg);color:var(--working)} .k.attention{background:var(--attention_bg);color:var(--attention)}
.k.failed{background:var(--failed_bg);color:var(--failed)} .k.done{background:var(--done_bg);color:var(--done)}
.srch{margin-left:auto;width:220px;height:26px;border-radius:6px;border:1px solid var(--hairline);display:flex;align-items:center;gap:8px;padding:0 10px;color:var(--muted)}
.ib{width:28px;height:28px;border-radius:6px;display:flex;align-items:center;justify-content:center;color:var(--text2)}
.caption{display:flex;height:36px}.caption div{width:46px;display:flex;align-items:center;justify-content:center;color:var(--text2)}
.main{display:flex;height:604px}
.rail{width:44px;background:var(--app);border-right:1px solid var(--hairline);display:flex;flex-direction:column;align-items:center;padding-top:6px;gap:4px}
.rail .ib.on{background:var(--selected);color:var(--accent_fg)}
.q{width:292px;background:var(--sidebar);border-right:1px solid var(--hairline);display:flex;flex-direction:column;padding:8px}
.qh{display:flex;align-items:center;gap:6px;height:28px;font-size:12px;font-weight:700;color:var(--text2);padding:0 4px}
.qc{background:var(--panel);border:1px solid var(--hairline);border-radius:8px;padding:8px 10px;margin-bottom:6px;display:flex;flex-direction:column;gap:4px}
.qc.attention{border-left:3px solid var(--attention)} .qc.failed{border-left:3px solid var(--failed)}
.qc.sel{border-color:var(--focus);border-left-width:3px}
.qc .r{display:flex;align-items:center;gap:6px}
.qc .t{font-weight:600}
.qc .m{font-size:12px;color:var(--muted)}
.qc .act{display:flex;gap:6px;margin-top:2px}
.btn{height:24px;padding:0 10px;border-radius:5px;display:inline-flex;align-items:center;gap:5px;font-size:12px;font-weight:600;border:1px solid var(--strong);color:var(--text)}
.btn.pri{background:var(--accent);border-color:var(--accent);color:var(--on_accent)}
.ql{height:32px;display:flex;align-items:center;gap:8px;padding:0 6px;border-radius:6px}
.ql .m{font-size:12px;color:var(--muted)}
.tiles{flex:1;min-width:0;display:grid;grid-template-columns:1fr 1fr;grid-template-rows:1fr 1fr;gap:4px;padding:4px;background:var(--app)}
.tile{background:var(--term_bg);border:1px solid var(--hairline);border-radius:8px;display:flex;flex-direction:column;min-height:0;min-width:0;overflow:hidden}
.tile .th{height:32px;display:flex;align-items:center;gap:8px;padding:0 8px 0 10px;font-size:12px;color:var(--text2);background:var(--panel);border-bottom:1px solid var(--hairline)}
.tile .th .nm{font-weight:600;color:var(--text);font-size:13px}
.tile.attention{border-color:var(--attention)} .tile.attention .th{background:var(--attention_bg)}
.tile.working .th{box-shadow:inset 0 2px 0 var(--working)} .tile.done .th{box-shadow:inset 0 2px 0 var(--done)}
.tile pre{padding:8px 12px;overflow:hidden;flex:1}
.pillst{display:flex;align-items:center;gap:4px;height:20px;padding:0 8px;border-radius:10px;font-size:11px;font-weight:700}
.pillst.working{background:var(--working_bg);color:var(--working)} .pillst.attention{background:var(--attention);color:var(--app)}
.pillst.done{background:var(--done_bg);color:var(--done)} .pillst.idle{background:var(--hover);color:var(--muted)}
.prog{height:3px;border-radius:2px;background:var(--hover);overflow:hidden}.prog i{display:block;height:100%;background:var(--working)}
'''


def c_workspace(theme):
    d = 'C'
    I = lambda n, s=16, c='': icon(n, d, s, c)
    q = [f'''<div class="q"><div class="qh">{I('approval', 14, 's-attention')}응답 필요<span class="mu">2</span><span class="grow"></span>{I('filter', 14, 'mu')}</div>
<div class="qc attention sel"><div class="r">{I('bot', 14, 'mu')}<span class="t">Codex · 리뷰</span><span class="grow"></span><span class="m">12초</span></div>
<div class="m">터미널 넓은 글자 정렬 · 명령 실행 승인</div><div class="mono" style="font-size:11px;color:var(--text2)">$ cargo clippy -p pod-ui -- -D warnings</div>
<div class="act"><span class="btn pri">{I('check', 12)}이번만 허용</span><span class="btn">거부</span><span class="btn" style="border-color:transparent;color:var(--accent_fg)">열기</span></div></div>
<div class="qc attention"><div class="r">{I('bot', 14, 'mu')}<span class="t">Claude · 구현</span><span class="grow"></span><span class="m">3분</span></div>
<div class="m">리뷰 의견 전달 재시도 · 질문에 답해 주세요</div></div>
<div class="qc failed"><div class="r">{I('failed', 14, 's-failed')}<span class="t">Codex · 구현</span><span class="grow"></span><span class="m">9분</span></div>
<div class="m">LSP 지연 시작 · 실패 종료 (1)</div></div>
<div class="qh" style="margin-top:6px">{I('working', 14, 's-working')}작업 중<span class="mu">3</span></div>''']
    for name, task, age in [('Claude · 구현', '터미널 넓은 글자 정렬', '4분'), ('Claude · 구현', '알림 묶음 처리', '11분'), ('Codex · 구현', '설치 프로그램 서명', '2분')]:
        q.append(f'<div class="ql">{I("working", 14, "s-working")}<span class="grow ell">{name} <span class="m">· {task}</span></span><span class="m">{age}</span></div>')
    q.append(f'<div class="qh" style="margin-top:6px">{I("done", 14, "s-done")}완료<span class="mu">5</span><span class="grow"></span><span class="mu" style="font-weight:400">모두 보기</span></div>')
    for name, task in [('Claude · 문서', '터미널 넓은 글자 정렬'), ('셸', '리뷰 의견 전달 재시도')]:
        q.append(f'<div class="ql">{I("done", 14, "s-done")}<span class="grow ell">{name}</span><span class="m">{task}</span></div>')
    q.append('</div>')
    t = TASKS[0]
    tiles = ''
    for n, s, l, _, _ in t['workers']:
        tiles += f'''<div class="tile {s}"><div class="th">{I(STATUS_ICON[s], 16, 's-' + s)}<span class="nm">{e(n)}</span><span class="grow"></span>
<span class="pillst {s}">{e(l)}</span>{I('dots-three' if False else 'ellipsis', 16, 'mu')}</div><pre>{TERM[n]}</pre></div>'''
    body = f'''<div class="tb"><div class="logo">P</div><span style="font-weight:600">관제</span><span class="mu">pod · 작업 6</span>
<div class="sum"><span class="k attention">{I('approval', 14)}응답 필요 2</span><span class="k failed">{I('failed', 14)}실패 1</span><span class="k working">{I('working', 14)}작업 중 3</span><span class="k done">{I('done', 14)}완료 5</span></div>
<div class="srch">{I('search', 14)}<span class="grow">검색</span><span style="font-size:11px">Ctrl K</span></div><div class="ib">{I('bell')}</div>
<div class="caption"><div>{I('minus')}</div><div>{I('square', 14)}</div><div>{I('x')}</div></div></div>
<div class="main"><div class="rail"><div class="ib on">{I('grid', 18)}</div><div class="ib">{I('layers', 18)}</div><div class="ib">{I('file-diff', 18)}</div><div class="ib">{I('bot', 18)}</div><span class="grow"></span><div class="ib" style="margin-bottom:8px">{I('settings', 18)}</div></div>
{''.join(q)}<div class="col grow"><div class="row" style="height:34px;padding:0 10px;gap:8px;background:var(--app)">{I('layers', 16, 'mu')}<b style="font-weight:600">터미널 넓은 글자 정렬</b><span class="mu" style="font-size:12px">pod/wide-glyph · 터미널 4</span><span class="grow"></span>{I('grid', 16, 'mu')}{I('plus', 16, 'mu')}</div>
<div class="tiles" style="padding-top:0">{tiles}</div></div></div>'''
    return page(d, theme, body, C_CSS)


# ================================================================ render
JOBS = [
    ('A-quiet', 'workspace', a_workspace, ['dark', 'light', 'hc-dark']),
    ('A-quiet', 'review', a_review, ['dark', 'light']),
    ('A-quiet', 'agents', a_agents, ['dark', 'light']),
    ('A-quiet', 'components', a_components, ['dark', 'light']),
    ('B-layered', 'workspace', b_workspace, ['dark', 'light']),
    ('C-mission', 'workspace', c_workspace, ['dark', 'light']),
]


def main():
    from playwright.sync_api import sync_playwright
    made = []
    with sync_playwright() as p:
        browser = p.chromium.launch(channel='chrome')
        ctx = browser.new_context(viewport={'width': 960, 'height': 640}, device_scale_factor=1.5,
                                  reduced_motion='reduce', offline=True)
        pg = ctx.new_page()
        for folder, screen, fn, themes in JOBS:
            (OUT / folder / 'src').mkdir(parents=True, exist_ok=True)
            for theme in themes:
                stem = f'{theme}-{screen}'
                h = OUT / folder / 'src' / f'{stem}.html'
                h.write_text(fn(theme), encoding='utf-8')
                pg.goto(h.resolve().as_uri())
                pg.wait_for_timeout(150)
                png = OUT / folder / f'{stem}.png'
                pg.screenshot(path=str(png))
                made.append(png)
        browser.close()
    for m in made:
        print(m.relative_to(OUT))


if __name__ == '__main__':
    main()
