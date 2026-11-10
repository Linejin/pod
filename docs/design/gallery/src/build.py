"""Pod design-sample gallery: 12 new workspace samples (dark + light) + the 6 existing D4 directions.

python -I docs/design/gallery/src/build.py   (from the repo root; local Chrome/Edge only, no network)

Reuses the D4 renderers instead of re-inventing them:
  * docs/design/d4-claude/src/build.py  -> Lucide icons (ISC, already vendored), fixture TASKS/TERM, BASE_CSS,
    and Direction A's workspace renderer (accent variants 15/16).
  * docs/design/d4/{dark,light}-workspace.html (Codex A) -> re-rendered with a swapped :root palette (variants 17/18).
Render: 960x640 CSS px at DPR 1.5 = 1440x960 PNG, reduced motion, offline context.
Writes: ../images/*.png, ./html/*.html, ../index.html (data inlined into src/index.template.html), ../samples.md (from EXISTING/NEW below), contrast report on stdout.
"""
import json
import pathlib
import re
import shutil
import sys

sys.dont_write_bytecode = True  # keep d4-claude/src free of __pycache__

HERE = pathlib.Path(__file__).resolve().parent
GAL = HERE.parent
DESIGN = GAL.parent
IMG = GAL / 'images'
HTML = HERE / 'html'
sys.path.insert(0, str(DESIGN / 'd4-claude' / 'src'))
import build as cb  # noqa: E402  (Claude D4 renderer; main() is guarded)
from tokens import DIRECTIONS  # noqa: E402

TASKS, TERM, STATUS_ICON, task_state, e = cb.TASKS, cb.TERM, cb.STATUS_ICON, cb.task_state, cb.e


def I(name, size=16, cls='', style=''):
    return cb.icon(name, 'A', size, cls, style)


TERM['Codex · 구현'] = '''<span class=dim>codex</span>  pod/lsp-lazy · LSP 지연 시작
<span class=ok>•</span> 수정  crates/pod-lsp/src/client.rs
<span class=ok>•</span> 실행  cargo test -p pod-lsp
<span class=del>error[E0599]: no method named `spawn_lazy` found</span>
<span class=del>  --&gt; crates/pod-lsp/src/client.rs:88:14</span>
<span class=del>프로세스가 코드 1로 종료되었습니다.</span>'''
LABEL = {'working': '작업 중', 'attention': '응답 필요', 'done': '완료', 'idle': '대기', 'failed': '실패'}
FOCUS = TASKS[0]


# --------------------------------------------------------------------------- palettes
def pal(base, **over):
    p = dict(base)
    p.update(over)
    return p


AD, AL = DIRECTIONS['A']['dark'], DIRECTIONS['A']['light']

P = {}
P['dense'] = {
    'dark': pal(AD, app='#131416', sidebar='#17181B', panel='#1A1B1E', raised='#202226', overlay='#24262A', hover='#222428',
                active='#2A2C31', selected='#24324A', selected_quiet='#222428', hairline='#26282C', strong='#34373D',
                text='#D9DBDF', text2='#A9ADB4', muted='#9CA0A8', term_bg='#151618', term_fg='#D2D5DA', accent='#3B6ACB',
                accent_fg='#8AB0F5', working='#9CA0A8', done='#9CA0A8', idle='#9CA0A8'),
    'light': pal(AL, app='#F5F5F6', sidebar='#F0F0F2', panel='#FFFFFF', hover='#E8E8EB', active='#DEDFE3', selected='#DCE5F7',
                 selected_quiet='#E8E8EB', hairline='#E2E3E6', strong='#CDCFD4', text='#1D1F23', text2='#4D5058', muted='#5E626A',
                 term_bg='#FFFFFF', term_fg='#22252A', working='#5E626A', done='#5E626A', failed='#AE2F2F'),
}
P['islands'] = {
    'dark': pal(AD, app='#0C0D10', sidebar='#16181D', panel='#181A1F', raised='#20232A', overlay='#252831', hover='#20232A',
                active='#272A32', selected='#262D45', selected_quiet='#22252C', hairline='#23262D', strong='#323640',
                accent='#5468E0', accent_fg='#A3B0FF', focus='#8C9CFF', term_bg='#181A1F', muted='#979CA7'),
    'light': pal(AL, app='#DADDE3', sidebar='#FFFFFF', panel='#FFFFFF', raised='#FFFFFF', hover='#EEF0F4', active='#E4E7EE',
                 selected='#E6E9FB', selected_quiet='#F0F1F5', hairline='#E6E8EC', strong='#D0D4DC', accent='#4553C9',
                 accent_fg='#3946B8', focus='#4553C9', muted='#545A65',
                 working='#0F5D85', attention='#744A00', done='#155F3D', failed='#A32A2A'),
}
P['mono'] = {
    'dark': pal(AD, app='#0A0A0A', sidebar='#0A0A0A', panel='#0A0A0A', raised='#141414', overlay='#181818', hover='#161616',
                active='#1E1E1E', selected='#1A1A1A', selected_quiet='#141414', hairline='#1C1C1C', strong='#2E2E2E',
                text='#EDEDED', text2='#B4B4B4', muted='#8E8E8E', accent='#EDEDED', accent_fg='#FFFFFF', focus='#FFFFFF',
                on_accent='#0A0A0A', term_bg='#0A0A0A', term_fg='#DCDCDC', idle='#8E8E8E'),
    'light': pal(AL, app='#FFFFFF', sidebar='#FFFFFF', panel='#FFFFFF', raised='#FAFAFA', overlay='#FFFFFF', hover='#F3F3F3',
                 active='#EAEAEA', selected='#F1F1F1', selected_quiet='#F6F6F6', hairline='#EFEFEF', strong='#DCDCDC',
                 text='#111111', text2='#444444', muted='#6B6B6B', accent='#111111', accent_fg='#111111', focus='#111111',
                 on_accent='#FFFFFF', term_bg='#FFFFFF', term_fg='#1A1A1A', idle='#6B6B6B'),
}
P['warm'] = {
    'dark': pal(AD, app='#1A1816', sidebar='#1F1C19', panel='#24211E', raised='#2B2824', overlay='#302C28', hover='#2C2925',
                active='#35312C', selected='#3A2E26', selected_quiet='#2E2A26', hairline='#332F2A', strong='#45403A',
                text='#EDE6DB', text2='#C3B9AB', muted='#A69B8D', accent='#B8653A', accent_fg='#EDA27A', focus='#EDA27A',
                working='#8FB8C9', attention='#E3B56A', done='#9CC48E', failed='#EE9585', idle='#A69B8D',
                term_bg='#211E1B', term_fg='#E4DCCF', add_bg='#252B1F', del_bg='#36211D'),
    'light': pal(AL, app='#EFEAE1', sidebar='#E9E3D8', panel='#FBF8F3', raised='#FFFDF9', overlay='#FFFDF9', hover='#E3DCCF',
                 active='#DAD2C3', selected='#F1E0D2', selected_quiet='#E6DFD2', hairline='#DED6C8', strong='#CBC1B0',
                 text='#2A251F', text2='#564D43', muted='#685E52', accent='#A4532B', accent_fg='#93491F', focus='#A4532B',
                 working='#2C6178', attention='#835300', done='#3D6B2D', failed='#AD3626', idle='#685E52',
                 term_bg='#FBF8F3', term_fg='#2F2923', add_bg='#E9F0DE', del_bg='#F7E3DC', add_fg='#3D6B2D', del_fg='#A3331F'),
}
P['blocks'] = {
    'dark': pal(AD, app='#111317', sidebar='#14171B', panel='#171A1F', raised='#1E2228', overlay='#242830', hover='#1F232A',
                active='#272C34', selected='#1E2B3F', hairline='#252A31', strong='#343A44', term_bg='#171A1F'),
    'light': pal(AL, app='#EEF0F3', sidebar='#F4F5F7', panel='#F7F8FA', raised='#FFFFFF', hairline='#DFE2E7', strong='#CBD0D8',
                 term_bg='#FFFFFF'),
}
P['kanban'] = {
    'dark': pal(AD, app='#15171B', sidebar='#15171B', panel='#1A1D22', raised='#22262D', overlay='#272B33', hairline='#272B32',
                strong='#373C46', accent='#2F7F86', accent_fg='#7FCCD2', focus='#7FCCD2', selected='#1C3337', muted='#979CA6'),
    'light': pal(AL, app='#EDEFF2', sidebar='#EDEFF2', panel='#E4E7EB', raised='#FFFFFF', hairline='#D9DDE3', strong='#C7CCD4',
                 accent='#1F6E75', accent_fg='#1A6168', focus='#1F6E75', selected='#DCEEEF'),
}
P['review'] = {
    'dark': pal(AD, app='#15171A', panel='#1A1C20', term_bg='#141619'),
    'light': pal(AL, app='#F1F2F4', term_bg='#FAFAFB'),
}
P['access'] = {
    'dark': pal(AD, app='#101215', sidebar='#14171B', panel='#181B20', raised='#20242A', overlay='#242930', hover='#262B33',
                active='#2E343D', selected='#1D3354', selected_quiet='#262B33', hairline='#3A414C', strong='#6B7584',
                control='#9AA4B2', text='#F4F6F8', text2='#D4D9E0', muted='#BCC3CD', accent='#4C82F0', accent_fg='#A9C8FF',
                focus='#FFD24D', working='#8CCBF2', attention='#F2C46E', done='#7FD3A6', failed='#FFA39A', idle='#BCC3CD',
                term_bg='#14171B', term_fg='#EEF1F4'),
    'light': pal(AL, app='#F6F7F9', sidebar='#FFFFFF', panel='#FFFFFF', hover='#E9EDF3', active='#DDE3EC', selected='#D9E6FB',
                 hairline='#9AA3B0', strong='#5D6675', control='#4A5361', text='#0E1116', text2='#2A313C', muted='#3D4552',
                 accent='#1F56C4', accent_fg='#1A4BAE', focus='#1A4BAE', working='#0D5A86', attention='#6E4500', done='#155E3B',
                 failed='#A31D2B', term_bg='#FFFFFF', term_fg='#0E1116'),
}
# accent / surface variants of Claude A (15, 16)
P['a-teal'] = {
    'dark': pal(AD, app='#14181B', sidebar='#14181B', panel='#191E21', raised='#20262A', overlay='#252C30', hover='#232A2E',
                active='#2B3337', selected='#173336', selected_quiet='#232A2E', hairline='#262E32', strong='#364045',
                accent='#1F8A86', accent_hover='#1B7B77', accent_fg='#6FD0C9', focus='#6FD0C9', term_bg='#191E21', muted='#949AA3'),
    'light': pal(AL, app='#F0F3F4', sidebar='#F0F3F4', hover='#E3E9EB', active='#D8E0E3', selected='#D5EEEC',
                 selected_quiet='#E1E8EA', hairline='#DCE3E5', strong='#C3CED2', accent='#137A76', accent_hover='#106A67',
                 accent_fg='#0E6B67', focus='#137A76'),
}
P['a-violet'] = {
    'dark': pal(AD, app='#171717', sidebar='#171717', panel='#1C1C1D', raised='#232324', overlay='#28282A', hover='#262627',
                active='#2E2E30', selected='#2C2442', selected_quiet='#262627', hairline='#2A2A2C', strong='#3B3B3E',
                text='#E8E8E9', text2='#B1B1B5', muted='#929297', accent='#7353D6', accent_hover='#6747C7', accent_fg='#BBA6FF',
                focus='#BBA6FF', term_bg='#1C1C1D', term_fg='#DCDCDF'),
    'light': pal(AL, app='#F4F4F5', sidebar='#F4F4F5', hover='#E9E9EB', active='#DFDFE2', selected='#E9E2FB',
                 selected_quiet='#E7E7EA', hairline='#E3E3E6', strong='#CDCDD2', text='#1C1C1F', text2='#4D4D54',
                 muted='#606068', accent='#6342C8', accent_hover='#5636B0', accent_fg='#5535B5', focus='#6342C8', failed='#AE2F2F'),
}
# Codex A variants: replacement :root values keyed by its token names (17, 18)
CODEX_VARIANTS = {
    'codex-neutral': {
        'dark': dict(app='#171717', sidebar='#1C1C1D', panel='#202021', raised='#272728', overlay='#2B2B2D', hover='#303032',
                     pressed='#363638', selected='#2A3346', hairline='#363638', primary='#ECECEC', secondary='#C6C6C8',
                     muted='#B0B0B3', terminal='#1A1A1B', terminal_fg='#E4E4E6'),
        'light': dict(app='#F6F6F6', sidebar='#EFEFEF', panel='#FFFFFF', raised='#F9F9F9', hover='#E7E7E8', pressed='#DEDEE0',
                      selected='#E1E8F4', hairline='#DADADC', primary='#222224', secondary='#4A4A4F', muted='#57575D',
                      terminal='#FCFCFC', terminal_fg='#29292C'),
    },
    'codex-green': {
        'dark': dict(app='#161A19', sidebar='#1A1F1E', panel='#1E2423', raised='#252C2A', overlay='#29312F', hover='#2D3634',
                     pressed='#333D3B', selected='#1E3B34', hairline='#33403D', accent='#8FD8BE', button='#2A7A63',
                     button_hover='#24694F', button_pressed='#1F5C46', info='#8FD8BE', info_bg='#1E3B34', cursor='#A6E3CD',
                     terminal='#181D1C'),
        'light': dict(app='#F3F6F5', sidebar='#EBF0EE', hover='#E0E8E5', pressed='#D5E0DC', selected='#DCEEE7',
                      hairline='#D4DEDA', accent='#1D6B54', button='#1F6F57', button_hover='#195D49', button_pressed='#144C3B',
                      info='#1D6B54', info_bg='#DCEEE7', cursor='#1D6B54'),
    },
}


def vars_css(p):
    return ':root{' + ';'.join(f'--{k}:{v}' for k, v in p.items()) + '}'


def page(key, theme, body, css, palette=None):
    p = palette or P[key][theme]
    dark = theme == 'dark'
    shadow = 'rgba(0,0,0,.45)' if dark else 'rgba(16,24,40,.12)'
    return f'''<!doctype html><html lang="ko"><head><meta charset="utf-8"><title>Pod sample {key} {theme}</title><style>{vars_css(p)}
:root{{--shadow:{shadow}}}{cb.BASE_CSS}{css}</style></head><body class="pg-{key} {theme}">{body}</body></html>'''


def caption():
    return f'<div class="caption"><div>{I("minus")}</div><div>{I("square", 14)}</div><div>{I("x")}</div></div>'


def need(t):
    return sum(1 for w in t['workers'] if w[1] in ('attention', 'failed'))


SUMMARY = [('working', 3), ('attention', 2), ('failed', 1), ('done', 5)]


# --------------------------------------------------------------------------- 07 dense editor
def s_dense(theme):
    css = '''
body{font-size:12px;line-height:18px}
.tb{height:30px;display:flex;align-items:center;gap:10px;padding-left:10px;background:var(--app);border-bottom:1px solid var(--hairline);color:var(--text2)}
.tb .menu{display:flex;gap:12px;font-size:12px}
.tb .q{margin:0 auto;width:300px;height:22px;border:1px solid var(--hairline);border-radius:4px;display:flex;align-items:center;gap:6px;padding:0 8px;color:var(--muted);background:var(--panel)}
.caption div{width:40px}
.main{display:flex;height:588px}
.act{width:36px;background:var(--app);border-right:1px solid var(--hairline);display:flex;flex-direction:column;align-items:center;padding-top:4px;gap:2px;color:var(--muted)}
.act div{width:28px;height:28px;display:flex;align-items:center;justify-content:center;border-radius:3px}
.act div.on{color:var(--text);box-shadow:inset 2px 0 0 var(--text)}
.sb{width:216px;background:var(--sidebar);border-right:1px solid var(--hairline);overflow:hidden}
.h{height:24px;display:flex;align-items:center;padding:0 6px 0 10px;font-size:11px;font-weight:600;color:var(--muted);letter-spacing:.04em;gap:4px}
.r{height:22px;display:flex;align-items:center;gap:5px;padding-right:8px;color:var(--text2)}
.r.t{color:var(--text)}
.r.sel{background:var(--selected);color:var(--text);box-shadow:inset 0 0 0 1px var(--focus)}
.r .m{margin-left:auto;font-size:11px;color:var(--muted)}
.r .m.a{color:var(--attention)} .r .m.f{color:var(--failed)}
.r .ic{color:var(--muted)}
.r .ic.s-attention{color:var(--attention)} .r .ic.s-failed{color:var(--failed)}
.c{flex:1;min-width:0;display:flex;flex-direction:column}
.tabs{height:26px;display:flex;background:var(--app);border-bottom:1px solid var(--hairline)}
.tab{display:flex;align-items:center;gap:5px;padding:0 10px;border-right:1px solid var(--hairline);color:var(--muted)}
.tab.on{background:var(--panel);color:var(--text);box-shadow:inset 0 1px 0 var(--accent)}
.g{flex:1;display:grid;grid-template-columns:1fr 1fr;grid-template-rows:1fr 1fr 120px;gap:1px;background:var(--hairline);min-height:0}
.p{background:var(--term_bg);min-height:0;min-width:0;display:flex;flex-direction:column}
.p.wide{grid-column:1 / 3}
.ph{height:22px;display:flex;align-items:center;gap:6px;padding:0 8px;color:var(--text2);font-size:11px;border-bottom:1px solid var(--hairline)}
.ph b{color:var(--text);font-weight:600}
.p.f .ph{box-shadow:inset 0 -1px 0 var(--accent)}
.p pre{font-size:11px;line-height:15px;padding:4px 8px;overflow:hidden}
.insp{width:196px;border-left:1px solid var(--hairline);background:var(--sidebar)}
.kv{display:grid;grid-template-columns:62px 1fr;row-gap:2px;padding:4px 10px;font-size:11px;line-height:16px}
.kv span:nth-child(odd){color:var(--muted)}
.stb{height:22px;display:flex;align-items:center;gap:12px;padding:0 10px;font-size:11px;color:var(--text2);background:var(--app);border-top:1px solid var(--hairline)}
.stb span{display:flex;align-items:center;gap:4px}
'''
    side = [f'<div class="h">작업 · pod<span class="grow"></span>{I("plus", 13)}{I("filter", 13)}{I("ellipsis", 13)}</div>']
    for i, t in enumerate(TASKS):
        st = task_state(t)
        n = need(t)
        m = f'<span class="m {"f" if st == "failed" else "a"}">{"!" if st == "failed" else ""}{n}</span>' if n else f'<span class="m">{t["age"]}</span>'
        side.append(f'<div class="r t" style="padding-left:6px">{I("chevron-down" if t["open"] else "chevron-right", 12)}{I(STATUS_ICON[st], 12, "s-" + st)}<span class="ell">{e(t["title"])}</span>{m}</div>')
        if t['open']:
            for name, s, label, prog, age in t['workers']:
                sel = ' sel' if (t is FOCUS and name == 'Codex · 리뷰') else ''
                tail = f'<span class="m {"a" if s == "attention" else "f" if s == "failed" else ""}">{e(label) if s in ("attention", "failed") else age}</span>'
                side.append(f'<div class="r{sel}" style="padding-left:34px">{I(STATUS_ICON[s], 12, "s-" + s)}<span class="ell">{e(name)}</span>{tail}</div>')
    side.append(f'<div class="h" style="margin-top:6px">{I("chevron-right", 12)}docs-site<span class="grow"></span><span style="font-weight:400">1</span></div>')
    side.append(f'<div class="h" style="margin-top:6px">{I("chevron-down", 12)}최근 이벤트</div>')
    for ic, s, txt in [('approval', 'attention', 'Codex · 리뷰 승인 요청'), ('failed', 'failed', 'Codex · 구현 코드 1 종료'), ('done', 'done', 'Claude · 문서 완료')]:
        side.append(f'<div class="r" style="padding-left:20px">{I(ic, 12, "s-" + s)}<span class="ell">{txt}</span></div>')
    panes = ''
    for n, s, l, p, a in FOCUS['workers'][:2]:
        panes += f'<div class="p{" f" if n == "Codex · 리뷰" else ""}"><div class="ph">{I(STATUS_ICON[s], 12, "s-" + s)}<b>{e(n)}</b><span class="s-{s}">{l}</span><span class="grow"></span>{p} · {a}</div><pre>{TERM[n]}</pre></div>'
    for n, s, l, p, a in FOCUS['workers'][2:]:
        panes += f'<div class="p"><div class="ph">{I(STATUS_ICON[s], 12, "s-" + s)}<b>{e(n)}</b><span class="mu">{l}</span><span class="grow"></span>{p}</div><pre>{TERM[n]}</pre></div>'
    panes += f'<div class="p wide"><div class="ph"><b>출력</b><span class="mu">문제 0 · 로그</span><span class="grow"></span>{I("x", 12)}</div><pre><span class=dim>12:41:07</span> podd  attach 4 terminals (pod/wide-glyph)\n<span class=dim>12:41:19</span> hook  Codex · 리뷰 → <span class=warn>approval_requested</span> cargo clippy\n<span class=dim>12:42:02</span> hook  Codex · 구현 (pod/lsp-lazy) → <span class=del>exited 1</span>\n<span class=dim>12:42:30</span> hook  Claude · 문서 → <span class=ok>done</span> 1m12s</pre></div>'
    body = f'''<div class="tb"><b style="color:var(--text)">Pod</b><div class="menu"><span>파일</span><span>편집</span><span>보기</span><span>작업</span><span>터미널</span><span>도움말</span></div>
<div class="q">{I("search", 12)}<span class="grow">pod · 터미널 넓은 글자 정렬</span><span style="font-size:11px">Ctrl K</span></div>{caption()}</div>
<div class="main"><div class="act"><div class="on">{I("layers", 18)}</div><div>{I("file-diff", 18)}</div><div>{I("bot", 18)}</div><div>{I("search", 18)}</div><span class="grow"></span><div>{I("settings", 18)}</div></div>
<div class="sb">{"".join(side)}</div>
<div class="c"><div class="tabs"><div class="tab on">{I("terminal", 12)}터미널 4</div><div class="tab">{I("file-diff", 12)}리뷰</div><div class="tab">{I("file-diff", 12)}terminal.rs ●</div><span class="grow"></span><div class="tab" style="border:0">{I("grid", 13)}{I("split", 13)}</div></div>
<div class="g">{panes}</div></div>
<div class="insp"><div class="h">세부 정보</div><div class="kv"><span>이름</span><span>Codex · 리뷰</span><span>상태</span><span class="s-attention">승인 필요</span><span>명령</span><span class="mono">cargo clippy</span><span>프로그램</span><span>codex</span><span>브랜치</span><span>pod/wide-glyph</span><span>경과</span><span>12초</span><span>출처</span><span>hook · 1초 전</span></div>
<div class="h" style="margin-top:6px">동작</div><div class="kv" style="grid-template-columns:1fr;row-gap:4px"><span style="color:var(--accent_fg)">이번만 허용 (y)</span><span style="color:var(--text2)">항상 허용 (a)</span><span style="color:var(--text2)">거부 (n)</span></div></div></div>
<div class="stb"><span>{I("plug", 12)}podd</span><span>{I("git-branch", 12)}pod/wide-glyph</span><span class="grow"></span>{"".join(f'<span class="{"s-" + s if s in ("attention", "failed") else ""}">{I(STATUS_ICON[s], 12)}{n}</span>' for s, n in SUMMARY)}<span>터미널 12</span><span>UTF-8</span></div>'''
    return page('dense', theme, body, css)


# --------------------------------------------------------------------------- 08 islands
def s_islands(theme):
    css = '''
body{background:var(--app)}
.top{height:44px;display:flex;align-items:center;gap:10px;padding:0 0 0 14px}
.logo{width:22px;height:22px;border-radius:7px;background:var(--accent);color:#fff;font:700 12px/22px "Segoe UI";text-align:center}
.q{margin-left:auto;margin-right:auto;width:320px;height:30px;border-radius:15px;background:var(--panel);display:flex;align-items:center;gap:8px;padding:0 12px;color:var(--muted);box-shadow:0 1px 2px var(--shadow)}
.main{display:flex;gap:10px;padding:0 10px;height:550px}
.isl{background:var(--panel);border-radius:12px;box-shadow:0 1px 3px var(--shadow),0 0 0 1px var(--hairline);display:flex;flex-direction:column;min-width:0;min-height:0;overflow:hidden}
.sb{width:236px;padding:10px 8px}
.sh{display:flex;align-items:center;gap:6px;padding:0 6px 8px;font-weight:600}
.sh .ib{width:26px;height:26px;border-radius:7px;display:flex;align-items:center;justify-content:center;color:var(--text2)}
.tr{height:32px;border-radius:8px;display:flex;align-items:center;gap:9px;padding:0 10px;color:var(--text2)}
.tr.cur{background:var(--selected);color:var(--text)}
.tr .t{flex:1;min-width:0}
.wr{height:28px;display:flex;align-items:center;gap:8px;padding:0 10px 0 30px;color:var(--text2);border-radius:8px}
.wr.sel{background:var(--hover);color:var(--text)}
.chip{font-size:11px;line-height:18px;padding:0 7px;border-radius:9px;font-weight:600}
.chip.attention{background:var(--attention_bg);color:var(--attention)} .chip.failed{background:var(--failed_bg);color:var(--failed)}
.right{flex:1;display:flex;flex-direction:column;gap:10px;min-width:0}
.head{height:44px;flex:none;flex-direction:row;align-items:center;gap:10px;padding:0 8px 0 16px}
.seg{display:flex;gap:2px;background:var(--app);border-radius:9px;padding:3px;margin-left:auto}
.seg div{padding:2px 10px;border-radius:7px;color:var(--text2);font-size:12px;display:flex;align-items:center;gap:5px}
.seg div.on{background:var(--panel);color:var(--text);box-shadow:0 1px 2px var(--shadow)}
.grid{flex:1;display:grid;grid-template-columns:1fr 1fr;grid-template-rows:1fr 1fr;gap:10px;min-height:0}
.ph{height:36px;display:flex;align-items:center;gap:8px;padding:0 12px;color:var(--text2);font-size:12px}
.ph .nm{color:var(--text);font-weight:600;font-size:13px}
.isl.focus{box-shadow:0 0 0 2px var(--focus)}
.isl pre{padding:0 14px 10px;overflow:hidden;flex:1}
.bot{height:46px;display:flex;align-items:center;justify-content:center}
.stb{display:flex;gap:4px;padding:4px;border-radius:12px;background:var(--panel);box-shadow:0 1px 3px var(--shadow),0 0 0 1px var(--hairline);font-size:12px;color:var(--text2)}
.stb span{display:flex;align-items:center;gap:5px;padding:2px 10px;border-radius:8px}
.stb b{color:var(--text);font-weight:600}
'''
    side = [f'<div class="sh">작업<span class="mu" style="font-weight:400;font-size:12px">6</span><span class="grow"></span><div class="ib">{I("filter")}</div><div class="ib">{I("plus")}</div></div>']
    for t in TASKS:
        st = task_state(t)
        n = need(t)
        m = f'<span class="chip {st}">{n}</span>' if n else f'<span class="mu" style="font-size:12px">{t["age"]}</span>'
        side.append(f'<div class="tr{" cur" if t is FOCUS else ""}"><span class="dot d-{st}"></span><span class="t ell">{e(t["title"])}</span>{m}</div>')
        if t is FOCUS:
            for name, s, label, _, _ in t['workers']:
                sel = name == 'Codex · 리뷰'
                side.append(f'<div class="wr{" sel" if sel else ""}">{I(STATUS_ICON[s], 14, "s-" + s)}<span class="grow ell">{e(name)}</span>{f"<span class=s-{s} style=font-size:12px>{label}</span>" if sel else ""}</div>')
    cards = ''.join(f'''<div class="isl{" focus" if n == "Codex · 리뷰" else ""}"><div class="ph">{I(STATUS_ICON[s], 16, "s-" + s)}<span class="nm">{e(n)}</span><span class="s-{s}">{e(l)}</span><span class="grow"></span>{I("maximize", 14, "mu")}{I("ellipsis", 14, "mu")}</div><pre>{TERM[n]}</pre></div>''' for n, s, l, _, _ in FOCUS['workers'])
    body = f'''<div class="top"><div class="logo">P</div><b>pod</b><span class="mu">/ 터미널 넓은 글자 정렬</span><div class="q">{I("search", 14)}<span class="grow">작업, 터미널, 명령</span><span class="kbd">Ctrl K</span></div>{I("bell", 16, "t2")}{I("settings", 16, "t2", "margin-left:8px")}{caption()}</div>
<div class="main"><div class="isl sb">{"".join(side)}</div>
<div class="right"><div class="isl head"><b>터미널 넓은 글자 정렬</b><span class="mu" style="font-size:12px">pod/wide-glyph · 터미널 4</span><div class="seg"><div class="on">{I("grid", 14)}터미널</div><div>{I("file-diff", 14)}리뷰</div><div>{I("bot", 14)}에이전트</div></div>{I("plus", 16, "t2")}</div>
<div class="grid">{cards}</div></div></div>
<div class="bot"><div class="stb"><span>{I("plug", 14, "s-done")}podd</span><span>{I("git-branch", 14)}pod/wide-glyph</span>{"".join(f'<span>{I(STATUS_ICON[s], 14, "s-" + s)}<b>{n}</b></span>' for s, n in SUMMARY)}</div></div>'''
    return page('islands', theme, body, css)


# --------------------------------------------------------------------------- 09 minimal mono
def s_mono(theme):
    css = '''
body{font-size:13px}
.tb{height:44px;display:flex;align-items:center;gap:14px;padding-left:24px;color:var(--muted)}
.tb b{color:var(--text);font-weight:600;letter-spacing:.02em}
.main{display:flex;height:566px}
.sb{width:248px;padding:8px 0 0 24px}
.lbl{font-size:11px;letter-spacing:.08em;color:var(--muted);margin:14px 0 6px;text-transform:uppercase}
.tr{height:30px;display:flex;align-items:center;gap:10px;color:var(--text2);padding-right:16px}
.tr.cur{color:var(--text);font-weight:600}
.tr .n{margin-left:auto;font-size:12px;font-weight:600}
.wr{height:26px;display:flex;align-items:center;gap:10px;padding-left:18px;color:var(--muted);padding-right:16px}
.wr.sel{color:var(--text)}
.wr.sel:before{content:"";width:2px;height:14px;background:var(--text);margin-left:-12px;margin-right:10px}
.mk{width:6px;height:6px;border-radius:50%;flex:none}
.c{flex:1;display:grid;grid-template-columns:1.25fr 1fr;gap:40px;padding:8px 32px 0 16px;min-width:0}
.col2{display:flex;flex-direction:column;gap:28px;min-height:0}
.pn{display:flex;flex-direction:column;min-height:0}
.pn .h{display:flex;align-items:baseline;gap:10px;margin-bottom:10px}
.pn .h b{font-weight:600;font-size:14px}
.pn pre{overflow:hidden}
.req{margin-top:14px;display:flex;gap:16px;align-items:center;font-size:12px}
.req .k{color:var(--text);font-weight:600;border-bottom:1px solid var(--text)}
.stb{height:30px;display:flex;align-items:center;gap:18px;padding:0 24px;font-size:12px;color:var(--muted)}
.stb .on{color:var(--text)}
'''
    side = ['<div class="lbl">pod · 작업 6</div>']
    for t in TASKS:
        st = task_state(t)
        n = need(t)
        mk = f'<span class="n s-{st}">{n}</span>' if n else f'<span class="n mu" style="font-weight:400">{t["age"]}</span>'
        side.append(f'<div class="tr{" cur" if t is FOCUS else ""}"><span class="ell">{e(t["title"])}</span>{mk}</div>')
        if t is FOCUS:
            for name, s, label, _, _ in t['workers']:
                sel = name == 'Codex · 리뷰'
                tag = f'<span class="s-{s}" style="margin-left:auto;font-size:12px">{label}</span>' if s in ('attention', 'failed') else ''
                side.append(f'<div class="wr{" sel" if sel else ""}"><span class="ell">{e(name)}</span>{tag}</div>')
    side.append('<div class="lbl" style="margin-top:22px">docs-site · 작업 1</div>')
    w = {n: (s, l, p, a) for n, s, l, p, a in FOCUS['workers']}

    def pn(n, extra=''):
        s, l, p, a = w[n]
        return f'<div class="pn"><div class="h"><b>{e(n)}</b><span class="{"s-" + s if s in ("attention", "failed", "working") else "mu"}">{l}</span><span class="mu">{p} · {a}</span></div><pre>{TERM[n]}</pre>{extra}</div>'
    req = '<div class="req"><span class="s-attention">승인 요청</span><span class="k">이번만 허용</span><span class="mu">항상 허용</span><span class="mu">거부</span></div>'
    body = f'''<div class="tb"><b>pod</b><span>터미널 넓은 글자 정렬</span><span class="grow"></span><span>Ctrl K 검색</span>{caption()}</div>
<div class="main"><div class="sb">{"".join(side)}</div>
<div class="c">{pn("Codex · 리뷰", req)}<div class="col2">{pn("Claude · 구현")}{pn("Claude · 문서")}</div></div></div>
<div class="stb"><span class="on">podd 연결됨</span><span>pod/wide-glyph</span><span class="grow"></span><span>작업 중 3</span><span class="s-attention">응답 2</span><span class="s-failed">실패 1</span><span>완료 5</span></div>'''
    return page('mono', theme, body, css)


# --------------------------------------------------------------------------- 10 warm neutral
def s_warm(theme):
    css = '''
body{font-size:13px}
.tb{height:40px;display:flex;align-items:center;gap:10px;padding-left:14px;background:var(--app)}
.logo{width:20px;height:20px;border-radius:50%;background:var(--accent);color:#fff;font:700 11px/20px "Segoe UI";text-align:center}
.q{margin:0 auto;width:300px;height:28px;border-radius:8px;background:var(--panel);border:1px solid var(--hairline);display:flex;align-items:center;gap:8px;padding:0 10px;color:var(--muted)}
.main{display:flex;height:572px}
.sb{width:244px;background:var(--sidebar);border-radius:0 12px 0 0;padding:12px 8px}
.sh{font:600 15px/22px "Segoe UI Variable Display","Segoe UI","Malgun Gothic";padding:0 8px 10px;display:flex;align-items:center}
.tr{height:32px;border-radius:8px;display:flex;align-items:center;gap:9px;padding:0 10px;color:var(--text2)}
.tr.cur{background:var(--panel);color:var(--text);box-shadow:0 1px 2px var(--shadow)}
.wr{height:28px;display:flex;align-items:center;gap:8px;padding:0 10px 0 30px;color:var(--text2);border-radius:8px}
.wr.sel{background:var(--selected);color:var(--text)}
.chip{font-size:11px;line-height:18px;padding:0 7px;border-radius:9px;font-weight:600}
.chip.attention{background:var(--attention_bg);color:var(--attention)} .chip.failed{background:var(--failed_bg);color:var(--failed)}
.c{flex:1;display:flex;flex-direction:column;padding:0 12px 0 12px;min-width:0}
.ttl{height:52px;display:flex;align-items:center;gap:12px}
.ttl h1{font:600 18px/26px "Segoe UI Variable Display","Segoe UI","Malgun Gothic"}
.pills{display:flex;gap:6px;margin-left:auto}
.pills span{padding:3px 12px;border-radius:14px;color:var(--text2);font-size:12px;display:flex;gap:6px;align-items:center}
.pills span.on{background:var(--text);color:var(--app)}
.lay{flex:1;display:grid;grid-template-columns:1.35fr 1fr;grid-template-rows:1fr 1fr 1fr;gap:10px;min-height:0;padding-bottom:10px}
.pn{background:var(--term_bg);border:1px solid var(--hairline);border-radius:10px;display:flex;flex-direction:column;min-height:0;min-width:0;overflow:hidden}
.pn.big{grid-row:1 / 4}
.pn.focus{border-color:var(--accent);box-shadow:0 0 0 1px var(--accent)}
.ph{height:34px;display:flex;align-items:center;gap:8px;padding:0 12px;color:var(--text2);font-size:12px}
.ph b{color:var(--text);font-weight:600;font-size:13px}
.pn pre{padding:0 14px 10px;overflow:hidden}
.ask{margin:auto 12px 12px;padding:10px 12px;border-radius:8px;background:var(--attention_bg);display:flex;gap:10px;align-items:center;font-size:12px}
.btn{height:28px;padding:0 12px;border-radius:7px;display:inline-flex;align-items:center;gap:6px;font-size:12px;font-weight:600;border:1px solid var(--strong);color:var(--text)}
.btn.pri{background:var(--accent);border-color:var(--accent);color:#fff}
.stb{height:28px;display:flex;align-items:center;gap:14px;padding:0 14px;font-size:12px;color:var(--text2);background:var(--app)}
.stb span{display:flex;gap:5px;align-items:center}
'''
    side = [f'<div class="sh">작업 공간<span class="grow"></span>{I("plus", 16, "t2")}</div>']
    for t in TASKS:
        st = task_state(t)
        n = need(t)
        m = f'<span class="chip {st}">{n}</span>' if n else f'<span class="mu" style="font-size:12px">{t["age"]}</span>'
        side.append(f'<div class="tr{" cur" if t is FOCUS else ""}"><span class="dot d-{st}"></span><span class="grow ell">{e(t["title"])}</span>{m}</div>')
        if t is FOCUS:
            for name, s, label, _, _ in t['workers']:
                side.append(f'<div class="wr{" sel" if name == "Codex · 리뷰" else ""}">{I(STATUS_ICON[s], 14, "s-" + s)}<span class="grow ell">{e(name)}</span></div>')
    w = {n: (s, l) for n, s, l, _, _ in FOCUS['workers']}
    big = f'''<div class="pn big focus"><div class="ph">{I("approval", 16, "s-attention")}<b>Codex · 리뷰</b><span class="s-attention">승인 필요</span><span class="grow"></span>{I("ellipsis", 14)}</div><pre>{TERM["Codex · 리뷰"]}</pre>
<div class="ask">{I("approval", 16, "s-attention")}<span class="grow"><b style="font-weight:600">cargo clippy</b> 실행을 허용할까요?</span><span class="btn">거부</span><span class="btn pri">이번만 허용</span></div></div>'''
    small = ''.join(f'<div class="pn"><div class="ph">{I(STATUS_ICON[w[n][0]], 14, "s-" + w[n][0])}<b>{e(n)}</b><span class="s-{w[n][0]}">{w[n][1]}</span></div><pre>{TERM[n]}</pre></div>' for n in ('Claude · 구현', 'Claude · 문서', '셸'))
    body = f'''<div class="tb"><div class="logo">P</div><b>pod</b><div class="q">{I("search", 14)}<span class="grow">찾기</span><span class="kbd">Ctrl K</span></div>{I("bell", 16, "t2")}{caption()}</div>
<div class="main"><div class="sb">{"".join(side)}</div><div class="c"><div class="ttl"><h1>터미널 넓은 글자 정렬</h1><span class="mu">pod/wide-glyph</span>
<div class="pills"><span class="on">{I("grid", 14)}터미널</span><span>{I("file-diff", 14)}리뷰</span><span>{I("bot", 14)}에이전트</span></div></div><div class="lay">{big}{small}</div></div></div>
<div class="stb"><span>{I("plug", 14, "s-done")}podd</span><span>{I("git-branch", 14)}pod/wide-glyph</span><span class="grow"></span>{"".join(f'<span class="s-{s}">{I(STATUS_ICON[s], 14)}{LABEL[s]} {n}</span>' for s, n in SUMMARY)}</div>'''
    return page('warm', theme, body, css)


# --------------------------------------------------------------------------- 11 block terminal
def s_blocks(theme):
    css = '''
.tb{height:36px;display:flex;align-items:center;gap:8px;padding-left:12px;background:var(--app);border-bottom:1px solid var(--hairline)}
.logo{width:18px;height:18px;border-radius:5px;background:var(--accent);color:#fff;font:700 11px/18px "Segoe UI";text-align:center}
.main{display:flex;height:580px}
.sb{width:228px;background:var(--sidebar);border-right:1px solid var(--hairline);padding:8px 6px}
.tr{height:28px;display:flex;align-items:center;gap:8px;padding:0 8px;border-radius:4px;color:var(--text2)}
.tr.cur{background:var(--selected);color:var(--text)}
.cnt{font-size:11px;line-height:16px;padding:0 5px;border-radius:8px;font-weight:600}
.cnt.attention{background:var(--attention_bg);color:var(--attention)} .cnt.failed{background:var(--failed_bg);color:var(--failed)}
.flt{display:flex;gap:4px;padding:8px 8px 4px;flex-wrap:wrap}
.flt span{font-size:11px;padding:0 8px;border-radius:10px;border:1px solid var(--hairline);color:var(--text2);display:flex;gap:4px;align-items:center}
.flt span.on{border-color:var(--accent_fg);color:var(--text)}
.c{flex:1;display:flex;flex-direction:column;min-width:0;background:var(--app)}
.ch{height:36px;display:flex;align-items:center;gap:10px;padding:0 14px;border-bottom:1px solid var(--hairline)}
.feed{flex:1;overflow:hidden;padding:10px 14px;display:flex;flex-direction:column;gap:8px}
.blk{background:var(--panel);border:1px solid var(--hairline);border-radius:6px;overflow:hidden;flex:none}
.bh{height:30px;display:flex;align-items:center;gap:8px;padding:0 10px;font-size:12px;color:var(--text2);background:var(--raised);border-bottom:1px solid var(--hairline)}
.bh .who{color:var(--text);font-weight:600}
.bh .cmd{font-family:"Cascadia Mono",Consolas,monospace;color:var(--text)}
.blk pre{padding:6px 12px 8px;font-size:12px;line-height:17px}
.blk.collapsed .bh{border-bottom:0;background:var(--panel)}
.blk.attention{border-color:var(--attention);box-shadow:inset 3px 0 0 var(--attention)}
.blk.failed{box-shadow:inset 3px 0 0 var(--failed)}
.acts{display:flex;gap:8px;padding:0 12px 10px}
.btn{height:26px;padding:0 10px;border-radius:4px;display:inline-flex;align-items:center;gap:6px;font-size:12px;font-weight:600;border:1px solid var(--strong);color:var(--text)}
.btn.pri{background:var(--accent);border-color:var(--accent);color:var(--on_accent)}
.inp{margin:0 14px 10px;height:40px;border:1px solid var(--control);border-radius:6px;background:var(--panel);display:flex;align-items:center;gap:8px;padding:0 10px;color:var(--muted)}
.stb{height:24px;display:flex;align-items:center;gap:12px;padding:0 10px;font-size:12px;color:var(--text2);background:var(--app);border-top:1px solid var(--hairline)}
.stb span{display:flex;gap:5px;align-items:center}
'''
    side = [f'<div class="row" style="padding:0 8px 6px;font-weight:600">작업<span class="grow"></span>{I("plus", 14, "t2")}</div>']
    for t in TASKS:
        st = task_state(t)
        n = need(t)
        m = f'<span class="cnt {st}">{n}</span>' if n else f'<span class="mu" style="font-size:12px">{t["age"]}</span>'
        side.append(f'<div class="tr{" cur" if t is FOCUS else ""}"><span class="dot d-{st}"></span><span class="grow ell">{e(t["title"])}</span>{m}</div>')
    side.append('<div class="flt" style="margin-top:10px"><span class="on">모든 에이전트 4</span><span>Claude · 구현</span><span>Codex · 리뷰</span><span>Claude · 문서</span><span>셸</span></div>')

    def blk(who, s, cmd, when, out, cls='', acts='', collapsed=False):
        head = f'<div class="bh">{I("chevron-right" if collapsed else "chevron-down", 12, "mu")}{I(STATUS_ICON[s], 14, "s-" + s)}<span class="who">{who}</span><span class="cmd">{cmd}</span><span class="grow"></span><span>{when}</span>{I("ellipsis", 14, "mu")}</div>'
        return f'<div class="blk {cls}{" collapsed" if collapsed else ""}">{head}{"" if collapsed else f"<pre>{out}</pre>"}{acts}</div>'
    feed = ''.join([
        blk('Claude · 문서', 'done', 'docs/06-terminal-view.md 갱신', '1분 12초 · 완료', '', collapsed=True),
        blk('Claude · 구현', 'done', 'cargo test -p pod-ui wide_glyph', '3.1초 · 종료 0', '', collapsed=True),
        blk('Claude · 구현', 'working', '셀 경계 테스트 추가', '42초째', '<span class=ok>•</span> 수정  crates/pod-ui/tests/wide_glyph.rs\n    <span class=add>+  #[test] fn hangul_takes_two_cells() {</span>\n    <span class=add>+      assert_eq!(layout("한").cols, 2);</span>\n<span class=info>⠋</span> 테스트 작성 중 <span class=dim>· esc 중단</span>'),
        blk('Codex · 리뷰', 'attention', '$ cargo clippy -p pod-ui -- -D warnings', '승인 대기 12초',
            '변경 요약  terminal.rs 셀 폭 계산 · status.rs 스피너 폭 고정\n<span class=warn>명령 실행 승인이 필요합니다.</span>', 'attention',
            f'<div class="acts"><span class="btn pri">{I("check", 14)}이번만 허용 <span class="kbd" style="color:inherit;border-color:rgba(255,255,255,.4)">Y</span></span><span class="btn">항상 허용 <span class="kbd">A</span></span><span class="btn">거부 <span class="kbd">N</span></span></div>'),
        blk('셸', 'idle', 'git status --short', '방금 · 종료 0', ' <span class=warn>M</span> crates/pod-ui/src/terminal.rs\n <span class=warn>M</span> crates/pod-ui/src/status.rs\n<span class=add>??</span> crates/pod-ui/tests/wide_glyph.rs'),
    ])
    body = f'''<div class="tb"><div class="logo">P</div><b>pod</b><span class="mu">/ 터미널 넓은 글자 정렬</span><span class="grow"></span>{I("search", 16, "t2")}{I("bell", 16, "t2", "margin-left:8px")}{caption()}</div>
<div class="main"><div class="sb">{"".join(side)}</div><div class="c"><div class="ch"><b>터미널 넓은 글자 정렬</b><span class="mu" style="font-size:12px">블록 5 · 에이전트 4 · 시간순</span><span class="grow"></span>
<span class="cnt attention">응답 필요 1</span>{I("filter", 16, "t2")}{I("split", 16, "t2")}</div><div class="feed">{feed}</div>
<div class="inp">{I("terminal", 14)}<span class="grow">Claude · 구현에게 입력 · Tab으로 대상 변경</span><span class="kbd">Enter</span></div></div></div>
<div class="stb"><span>{I("plug", 14, "s-done")}podd</span><span>{I("git-branch", 14)}pod/wide-glyph</span><span class="grow"></span>{"".join(f'<span>{I(STATUS_ICON[s], 14, "s-" + s)}{n}</span>' for s, n in SUMMARY)}</div>'''
    return page('blocks', theme, body, css)


# --------------------------------------------------------------------------- 12 kanban
def s_kanban(theme):
    css = '''
.tb{height:40px;display:flex;align-items:center;gap:10px;padding-left:14px;border-bottom:1px solid var(--hairline)}
.logo{width:20px;height:20px;border-radius:6px;background:var(--accent);color:#fff;font:700 11px/20px "Segoe UI";text-align:center}
.tabs{display:flex;gap:2px;margin-left:10px}
.tabs span{padding:4px 10px;border-radius:6px;color:var(--text2);display:flex;gap:6px;align-items:center}
.tabs span.on{background:var(--selected);color:var(--text)}
.bar{height:40px;display:flex;align-items:center;gap:10px;padding:0 14px}
.fchip{height:26px;padding:0 10px;border-radius:13px;border:1px solid var(--hairline);display:flex;gap:6px;align-items:center;font-size:12px;color:var(--text2)}
.board{display:grid;grid-template-columns:1fr 1fr 1fr;gap:12px;padding:0 14px;height:420px}
.colm{background:var(--panel);border-radius:10px;padding:8px;display:flex;flex-direction:column;gap:8px;min-height:0;overflow:hidden}
.colh{display:flex;align-items:center;gap:8px;padding:2px 4px 4px;font-weight:600}
.colh .n{font-weight:400;color:var(--muted);font-size:12px}
.card{background:var(--raised);border:1px solid var(--hairline);border-radius:8px;padding:8px 10px;display:flex;flex-direction:column;gap:4px;position:relative}
.card .t{display:flex;align-items:center;gap:6px;font-weight:600}
.card .m{font-size:12px;color:var(--muted);display:flex;gap:6px;align-items:center}
.card .last{font:11px/16px "Cascadia Mono",Consolas,monospace;color:var(--text2);background:var(--term_bg);border-radius:4px;padding:3px 6px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.card.attention{box-shadow:inset 3px 0 0 var(--attention)} .card.failed{box-shadow:inset 3px 0 0 var(--failed)}
.card.drag{transform:rotate(-2deg) translate(6px,-4px);box-shadow:0 12px 28px var(--shadow),0 0 0 2px var(--focus);z-index:3}
.drop{height:40px;flex:none;border:1.5px dashed var(--strong);border-radius:8px;display:flex;align-items:center;justify-content:center;color:var(--muted);font-size:12px}
.grip{position:absolute;right:8px;top:9px;width:8px;height:12px;background-image:radial-gradient(var(--muted) 1px,transparent 1.3px);background-size:4px 4px;opacity:.8}
.acts{display:flex;gap:6px;margin-top:2px}
.btn{height:24px;padding:0 9px;border-radius:5px;display:inline-flex;align-items:center;gap:5px;font-size:12px;font-weight:600;border:1px solid var(--strong);color:var(--text)}
.btn.pri{background:var(--accent);border-color:var(--accent);color:#fff}
.drawer{position:absolute;left:0;right:0;bottom:24px;height:136px;border-top:1px solid var(--hairline);background:var(--term_bg);display:flex;flex-direction:column}
.dh{height:28px;display:flex;align-items:center;gap:8px;padding:0 14px;font-size:12px;color:var(--text2);border-bottom:1px solid var(--hairline)}
.drawer pre{padding:4px 14px;overflow:hidden}
.stb{position:absolute;left:0;right:0;bottom:0;height:24px;display:flex;align-items:center;gap:12px;padding:0 12px;font-size:12px;color:var(--text2);border-top:1px solid var(--hairline)}
.stb span{display:flex;gap:5px;align-items:center}
'''

    def card(task, who, s, last, extra='', cls=''):
        return f'''<div class="card {s} {cls}"><span class="grip"></span><div class="t">{I(STATUS_ICON[s], 14, "s-" + s)}<span class="ell">{e(who)}</span></div>
<div class="m"><span class="ell">{e(task)}</span></div><div class="last">{last}</div>{extra}</div>'''
    working = [card('터미널 넓은 글자 정렬', 'Claude · 구현', 'working', '⠋ 셀 경계 테스트 추가 중 · 42초'),
               card('알림 묶음 처리', 'Claude · 구현', 'working', '• 수정 crates/pod-app/src/notify.rs'),
               '<div class="drop">순서 바꾸기 · 여기에 놓기</div>',
               card('설치 프로그램 서명', 'Codex · 구현', 'working', 'signtool verify /pa target\\pod.exe', cls='drag')]
    attention = [card('터미널 넓은 글자 정렬', 'Codex · 리뷰', 'attention', '$ cargo clippy -p pod-ui -- -D warnings',
                      f'<div class="acts"><span class="btn pri">이번만 허용</span><span class="btn">거부</span><span class="grow"></span><span class="m">12초</span></div>'),
                 card('LSP 지연 시작', 'Codex · 구현', 'failed', 'error[E0599]: no method `spawn_lazy`',
                      f'<div class="acts"><span class="btn">{I("terminal", 12)}로그</span><span class="btn">{I("refresh", 12)}다시 실행</span><span class="grow"></span><span class="m s-failed">코드 1</span></div>'),
                 card('리뷰 의견 전달 재시도', 'Claude · 구현', 'attention', '어느 브랜치에 의견을 보낼까요? (1/2)')]
    done = [card('터미널 넓은 글자 정렬', 'Claude · 문서', 'done', '작업 완료 · 1분 12초 · 토큰 18.2k'),
            card('설정 화면 정리', 'Codex · 구현', 'done', '✓ 6 files changed'),
            card('리뷰 의견 전달 재시도', '셸', 'done', 'PS C:\\src\\pod> 종료 0')]

    def col(s, title, items, n):
        return f'<div class="colm"><div class="colh">{I(STATUS_ICON[s], 16, "s-" + s)}{title}<span class="n">{n}</span><span class="grow"></span>{I("ellipsis", 14, "mu")}</div>{"".join(items)}</div>'
    body = f'''<div class="tb"><div class="logo">P</div><b>pod</b><div class="tabs"><span class="on">{I("grid", 14)}보드</span><span>{I("terminal", 14)}터미널</span><span>{I("file-diff", 14)}리뷰</span></div>
<span class="grow"></span>{I("search", 16, "t2")}{I("bell", 16, "t2", "margin-left:8px")}{caption()}</div>
<div class="bar"><b>모든 작업</b><span class="mu" style="font-size:12px">에이전트 9 · 작업 6</span><span class="grow"></span><span class="fchip">{I("folder-git", 12)}pod</span><span class="fchip">{I("bot", 12)}Claude · Codex</span><span class="fchip">{I("filter", 12)}필터</span></div>
<div class="board">{col("working", "작업 중", working, 3)}{col("attention", "응답 필요", attention, 3)}{col("done", "완료", done, 5)}</div>
<div class="drawer"><div class="dh">{I("approval", 14, "s-attention")}<b style="color:var(--text)">Codex · 리뷰</b><span>터미널 넓은 글자 정렬 · pod/wide-glyph</span><span class="grow"></span>{I("maximize", 14)}{I("x", 14)}</div><pre>{TERM["Codex · 리뷰"]}</pre></div>
<div class="stb"><span>{I("plug", 14, "s-done")}podd</span><span class="grow"></span><span>카드를 끌어 상태 대신 담당·순서를 정리합니다 · 상태는 에이전트가 보고</span></div>'''
    return page('kanban', theme, body, css)


# --------------------------------------------------------------------------- 13 review-centric
def s_review(theme):
    css = '''
.tb{height:36px;display:flex;align-items:center;gap:8px;padding-left:12px;background:var(--app);border-bottom:1px solid var(--hairline)}
.logo{width:18px;height:18px;border-radius:5px;background:var(--accent);color:#fff;font:700 11px/18px "Segoe UI";text-align:center}
.rvh{height:40px;display:flex;align-items:center;gap:10px;padding:0 12px;background:var(--panel);border-bottom:1px solid var(--hairline)}
.seg{display:inline-flex;height:26px;border-radius:5px;background:var(--app);border:1px solid var(--hairline);padding:2px;gap:2px}
.seg div{display:flex;align-items:center;gap:5px;padding:0 8px;border-radius:3px;color:var(--text2);font-size:12px}
.seg div.on{background:var(--raised);color:var(--text);box-shadow:0 0 0 1px var(--strong)}
.btn{height:28px;padding:0 12px;border-radius:4px;display:inline-flex;align-items:center;gap:6px;font-size:13px;font-weight:600;border:1px solid var(--strong);color:var(--text)}
.btn.pri{background:var(--accent);border-color:var(--accent);color:var(--on_accent)}
.body{display:flex;height:364px}
.fl{width:200px;border-right:1px solid var(--hairline);padding:6px;background:var(--panel)}
.f{height:26px;display:flex;align-items:center;gap:6px;padding:0 6px;border-radius:4px;color:var(--text2);font-size:12px}
.f.sel{background:var(--selected);color:var(--text)}
.sbs{flex:1;display:grid;grid-template-columns:1fr 1fr;min-width:0;font:12px/20px "Cascadia Mono",Consolas,"Malgun Gothic",monospace;background:var(--panel)}
.side{overflow:hidden;border-right:1px solid var(--hairline)}
.fh{height:28px;display:flex;align-items:center;gap:6px;padding:0 10px;font:12px/20px "Segoe UI","Malgun Gothic";color:var(--text2);border-bottom:1px solid var(--hairline);background:var(--app)}
.l{display:flex;height:20px;white-space:pre}
.l .n{width:36px;text-align:right;padding-right:8px;color:var(--muted);font-size:11px;flex:none}
.l .x{flex:1;color:var(--term_fg);overflow:hidden}
.l.add{background:var(--add_bg)} .l.del{background:var(--del_bg)} .l.emp{background:repeating-linear-gradient(135deg,transparent 0 4px,var(--hairline) 4px 5px)}
.cm{width:260px;border-left:1px solid var(--hairline);background:var(--panel);padding:8px 10px;display:flex;flex-direction:column;gap:8px}
.card{border:1px solid var(--strong);border-radius:6px;padding:8px 10px;font-size:12px;line-height:18px;box-shadow:inset 3px 0 0 var(--attention)}
.inline{margin:2px 10px 2px 44px;padding:6px 10px;border-radius:6px;border:1px solid var(--strong);background:var(--raised);font:12px/18px "Segoe UI","Malgun Gothic";white-space:normal;color:var(--text)}
.dock{height:156px;border-top:1px solid var(--hairline);display:flex;flex-direction:column;background:var(--app)}
.dh{height:28px;display:flex;align-items:center;gap:8px;padding:0 10px;font-size:12px;color:var(--text2)}
.mini{flex:1;display:grid;grid-template-columns:repeat(4,1fr);gap:1px;background:var(--hairline);min-height:0}
.mt{background:var(--term_bg);display:flex;flex-direction:column;min-width:0}
.mh{height:24px;display:flex;align-items:center;gap:6px;padding:0 8px;font-size:12px;color:var(--text2);border-bottom:1px solid var(--hairline)}
.mh b{color:var(--text);font-weight:600}
.mt pre{font-size:11px;line-height:15px;padding:4px 8px;overflow:hidden}
.mt.attention{box-shadow:inset 0 2px 0 var(--attention)} .mt.failed{box-shadow:inset 0 2px 0 var(--failed)}
.stb{height:24px;display:flex;align-items:center;gap:12px;padding:0 10px;font-size:12px;color:var(--text2);border-top:1px solid var(--hairline);background:var(--app)}
.stb span{display:flex;gap:5px;align-items:center}
'''
    files = ''.join(f'<div class="f{" sel" if i == 0 else ""}">{I("file-diff", 14)}<span class="grow ell">{n}</span><span style="color:var(--add_fg)">{a}</span><span style="color:var(--del_fg)">{d}</span></div>' for i, (n, a, d) in enumerate([('terminal.rs', '+24', '−6'), ('status.rs', '+3', '−1'), ('wide_glyph.rs', '+58', '')]))
    left = [('212', '    for (col, cell) in row.cells().enumerate() {', ''), ('213', '        let ch = cell.ch;', ''), ('214', '        let w = glyph.advance;', 'del'),
            ('', '', 'emp'), ('', '', 'emp'), ('215', '        let x = self.origin.x + col as f32 * ...;', ''), ('216', '        self.quads.push(Quad::glyph(x, w, ch));', ''), ('', '', 'emp'), ('217', '    }', '')]
    right = [('212', '    for (col, cell) in row.cells().enumerate() {', ''), ('213', '        let ch = cell.ch;', ''), ('214', '        // 넓은 글자는 두 칸을 차지한다', 'add'),
             ('215', '        let cols = width_of(ch).max(1);', 'add'), ('216', '        let w = self.cell_w * cols as f32;', 'add'), ('217', '        let x = self.origin.x + col as f32 * ...;', ''),
             ('218', '        self.quads.push(Quad::glyph(x, w, ch));', ''), ('219', '        if cols == 2 { self.skip_next = true; }', 'add'), ('220', '    }', '')]

    def side(rows, cm=False):
        out = []
        for i, (n, x, k) in enumerate(rows):
            out.append(f'<div class="l {k}"><span class="n">{n}</span><span class="x">{e(x)}</span></div>')
            if cm and i == 4:
                out.append('<div class="inline"><b>나</b> <span class="mu">· 216행</span><br>넓은 글자 폭은 매 프레임 계산하지 말고 캐시해 주세요.</div>')
        return ''.join(out)
    minis = ''
    for n, s, l, p, a in FOCUS['workers']:
        minis += f'<div class="mt {s}"><div class="mh">{I(STATUS_ICON[s], 12, "s-" + s)}<b>{e(n)}</b><span class="s-{s}">{l}</span></div><pre>{"<br>".join(TERM[n].splitlines()[-3:])}</pre></div>'
    body = f'''<div class="tb"><div class="logo">P</div><b>pod</b><span class="mu">/ 터미널 넓은 글자 정렬 / 리뷰</span><span class="grow"></span>{I("search", 16, "t2")}{caption()}</div>
<div class="rvh">{I("git-pr", 16, "acc")}<b>pod/wide-glyph → main</b><span class="mu" style="font-size:12px">파일 3 · <span style="color:var(--add_fg)">+85</span> <span style="color:var(--del_fg)">−7</span> · 의견 1</span><span class="grow"></span>
<div class="seg"><div class="on">{I("split", 14)}나란히</div><div>{I("unified", 14)}한 줄로</div></div>{I("arrow-up", 16, "t2")}{I("arrow-down", 16, "t2")}<span class="btn">{I("comment", 14)}의견</span><span class="btn pri">{I("send", 14)}에이전트에 보내기</span></div>
<div class="body"><div class="fl"><div class="mu" style="font-size:12px;padding:2px 6px 4px;font-weight:600">변경된 파일</div>{files}</div>
<div class="sbs"><div class="side"><div class="fh">{I("file-diff", 14)}terminal.rs <span class="mu">main</span></div>{side(left)}</div><div class="side" style="border:0"><div class="fh">{I("file-diff", 14)}terminal.rs <span class="mu">pod/wide-glyph</span></div>{side(right, True)}</div></div>
<div class="cm"><b>의견 <span class="mu" style="font-weight:400;font-size:12px">미해결 1</span></b><div class="card"><b>terminal.rs:216</b><br><span class="t2">넓은 글자 폭은 매 프레임 계산하지 말고 캐시해 주세요.</span></div>
<div class="banner warn" style="display:flex;gap:8px;padding:8px 10px;border-radius:6px;background:var(--attention_bg);font-size:12px;line-height:18px">{I("approval", 14, "s-attention")}<span>Codex · 리뷰가 clippy 실행 승인을 기다립니다.</span></div>
<div class="banner" style="display:flex;gap:8px;padding:8px 10px;border-radius:6px;background:var(--failed_bg);font-size:12px;line-height:18px">{I("failed", 14, "s-failed")}<span>LSP 지연 시작 · Codex · 구현 코드 1 종료</span></div></div></div>
<div class="dock"><div class="dh">{I("terminal", 14)}<b style="color:var(--text)">터미널 4</b><span>리뷰 중에도 상태와 마지막 줄을 표시</span><span class="grow"></span>{I("maximize", 14)}{I("chevron-down", 14)}</div><div class="mini">{minis}</div></div>
<div class="stb"><span>{I("plug", 14, "s-done")}podd</span><span>{I("git-branch", 14)}pod/wide-glyph</span><span class="grow"></span>{"".join(f'<span>{I(STATUS_ICON[s], 14, "s-" + s)}{n}</span>' for s, n in SUMMARY)}</div>'''
    return page('review', theme, body, css)


# --------------------------------------------------------------------------- 14 high-contrast / accessibility first
def s_access(theme):
    css = '''
body{font-size:15px;line-height:22px}
pre{font-size:14px;line-height:21px}
.tb{height:44px;display:flex;align-items:center;gap:12px;padding-left:14px;background:var(--app);border-bottom:2px solid var(--hairline)}
.logo{width:24px;height:24px;border-radius:6px;background:var(--accent);color:#fff;font:700 13px/24px "Segoe UI";text-align:center}
.tb .bt{height:36px;padding:0 12px;border-radius:6px;border:1.5px solid var(--strong);display:flex;align-items:center;gap:8px;font-size:14px}
.main{display:flex;height:564px}
.sb{width:272px;background:var(--sidebar);border-right:2px solid var(--hairline);padding:10px 8px;display:flex;flex-direction:column;gap:2px}
.tr{min-height:40px;display:flex;align-items:center;gap:10px;padding:0 10px;border-radius:6px;font-weight:600}
.tr.cur{background:var(--selected)}
.wr{min-height:36px;display:flex;align-items:center;gap:10px;padding:0 10px 0 34px;border-radius:6px;color:var(--text2)}
.wr.focus{outline:3px solid var(--focus);outline-offset:-3px;background:var(--hover);color:var(--text)}
.tag{font-size:13px;line-height:20px;padding:0 8px;border-radius:4px;border:1.5px solid currentColor;font-weight:600;white-space:nowrap}
.c{flex:1;display:flex;flex-direction:column;min-width:0;padding:12px;gap:12px}
.ask{border:2px solid var(--attention);border-radius:8px;padding:12px 14px;display:flex;align-items:center;gap:12px;background:var(--attention_bg)}
.btn{height:40px;min-width:40px;padding:0 16px;border-radius:6px;display:inline-flex;align-items:center;gap:8px;font-size:15px;font-weight:600;border:2px solid var(--strong);color:var(--text);background:var(--panel)}
.btn.pri{background:var(--accent);border-color:var(--accent);color:#fff}
.btn.focus{outline:3px solid var(--focus);outline-offset:2px}
.grid{flex:1;display:grid;grid-template-columns:1fr 1fr;gap:12px;min-height:0}
.pn{border:1.5px solid var(--hairline);border-radius:8px;background:var(--term_bg);display:flex;flex-direction:column;min-height:0;overflow:hidden}
.ph{height:40px;display:flex;align-items:center;gap:10px;padding:0 12px;border-bottom:1.5px solid var(--hairline);font-weight:600}
.pn pre{padding:8px 12px;overflow:hidden}
.stb{height:32px;display:flex;align-items:center;gap:16px;padding:0 14px;font-size:14px;background:var(--app);border-top:2px solid var(--hairline)}
.stb span{display:flex;gap:6px;align-items:center}
.u{text-decoration:underline;text-underline-offset:3px}
'''
    side = [f'<div class="row" style="padding:2px 10px 8px;font-weight:600;font-size:16px">작업 6<span class="grow"></span><span class="btn" style="height:36px;padding:0 10px;font-size:14px">{I("plus", 18)}새 작업</span></div>']
    for t in TASKS[:5]:
        st = task_state(t)
        tag = f'<span class="tag s-{st}">{"실패" if st == "failed" else "응답 " + str(need(t))}</span>' if need(t) else ''
        side.append(f'<div class="tr{" cur" if t is FOCUS else ""}">{I(STATUS_ICON[st], 18, "s-" + st)}<span class="grow ell">{e(t["title"])}</span>{tag}</div>')
        if t is FOCUS:
            for name, s, label, _, _ in t['workers']:
                side.append(f'<div class="wr{" focus" if name == "Codex · 리뷰" else ""}">{I(STATUS_ICON[s], 18, "s-" + s)}<span class="grow ell">{e(name)}</span><span class="s-{s}" style="font-size:13px">{label}</span></div>')
    panes = ''.join(f'<div class="pn"><div class="ph">{I(STATUS_ICON[s], 18, "s-" + s)}{e(n)}<span class="s-{s}" style="font-weight:400;font-size:14px">{l}</span><span class="grow"></span>{I("ellipsis", 18, "t2")}</div><pre>{TERM[n]}</pre></div>' for n, s, l, _, _ in FOCUS['workers'][:2])
    body = f'''<div class="tb"><div class="logo">P</div><b style="font-size:16px">pod</b><span class="t2">터미널 넓은 글자 정렬</span><span class="grow"></span><span class="bt">{I("search", 18)}검색 <span class="kbd" style="font-size:12px">Ctrl K</span></span><span class="bt">{I("bell", 18)}알림 3</span>{caption()}</div>
<div class="main"><div class="sb">{"".join(side)}</div><div class="c">
<div class="ask">{I("approval", 24, "s-attention")}<div class="grow"><b>Codex · 리뷰가 명령 실행 승인을 기다립니다</b><div class="t2" style="font-size:14px"><span class="mono">cargo clippy -p pod-ui -- -D warnings</span></div></div><span class="btn focus pri">{I("check", 18)}이번만 허용 (Y)</span><span class="btn">거부 (N)</span></div>
<div class="grid">{panes}</div>
<div class="ask" style="border-color:var(--failed);background:var(--failed_bg)">{I("failed", 24, "s-failed")}<div class="grow"><b>LSP 지연 시작 · Codex · 구현 실패 (코드 1)</b><div class="t2" style="font-size:14px">error[E0599]: no method named spawn_lazy</div></div><span class="btn">로그 보기</span></div></div></div>
<div class="stb"><span>{I("plug", 18, "s-done")}<span class="u">연결됨</span></span><span class="grow"></span>{"".join(f'<span class="s-{s}">{I(STATUS_ICON[s], 18)}{LABEL[s]} {n}</span>' for s, n in SUMMARY)}</div>'''
    return page('access', theme, body, css)


# --------------------------------------------------------------------------- 15/16 Claude A accent variants
def claude_a_variant(key):
    def fn(theme):
        saved = DIRECTIONS['A'][theme]
        DIRECTIONS['A'][theme] = P[key][theme]
        try:
            return cb.a_workspace(theme)
        finally:
            DIRECTIONS['A'][theme] = saved
    return fn


def _codex_ns():
    """Load only the pure markup section of docs/design/d4/build.py (icon paths, row(), btn(), css, document()).
    Its audit and render sections are skipped, so nothing under d4/ is rewritten."""
    src = (DESIGN / 'd4' / 'build.py').read_text(encoding='utf-8')
    ns = {'html': __import__('html')}
    exec(src[src.index('# Original schematic strokes'):src.index('def workspace():')], ns)
    exec(src[src.index('def document(mode,scene):'):src.index('def direction(mode,scene,variant):')], ns)
    names = re.search(r"names = '([^']+)'", src).group(1).split()
    rows = dict(re.findall(r"'(dark|light)': '([0-9A-F ]+)'", src))
    base = {m: dict(zip(names, ['#' + v for v in rows[m].split()])) for m in ('dark', 'light')}
    return ns, base


CX = {'working': 'work', 'attention': 'wait', 'done': 'done', 'idle': 'idle', 'failed': 'error'}


def _plain(t):
    import html as h
    return h.unescape(re.sub(r'<[^>]+>', '', t))


def codex_a_variant(key):
    """Codex A's own layout functions, fed the shared Pod fixture, with a swapped palette."""
    def fn(theme):
        ns, base = _codex_ns()
        row, btn, icon = ns['row'], ns['btn'], ns['icon']

        def sidebar():
            s = ('<aside><div class="sidehead"><b>작업 공간</b><span class="grow"></span>' + btn('plus', '새 작업') + btn('terminal', '새 터미널') + btn('more', '더 보기') +
                 '</div><div class="filter">' + icon('search') + '<span>작업, 터미널 검색</span></div><div class="section">pod <small>6</small><span class="grow"></span>' + btn('down') + '</div>')
            for t in TASKS[:4]:
                n = need(t)
                s += row(e(t['title']), CX[task_state(t)], task=True, meta=(('실패 ' if task_state(t) == 'failed' else '응답 ') + str(n)) if n else t['age'])
                if t['open']:
                    for name, st, label, _, _ in t['workers']:
                        sel = t is FOCUS and name == 'Codex · 리뷰'
                        s += row(e(name), CX[st], selected=sel, meta=label if (sel or st == 'failed') else '')
            return s + '<div class="sidefoot">' + icon('tree') + '에이전트<span class="grow"></span>' + btn('more') + '</div></aside>'

        def workspace():
            s = '<div class="bar"><b>터미널 넓은 글자 정렬</b><span class="grow"></span><small>pod/wide-glyph</small>' + btn('grid', '격자') + btn('more') + '</div><div class="grid">'
            for name, st, label, _, _ in FOCUS['workers']:
                cx = CX[st]
                s += ('<div class="terminal"><div class="panelhead">' + icon(cx, {'work': 'accent', 'wait': 'warning', 'done': 'success', 'idle': 'muted'}[cx]) +
                      e(name) + '<span class="grow"></span>' + btn('more') + '</div>')
                if cx == 'wait':
                    s += '<div class="banner">' + icon('wait') + '<span>승인이 필요합니다<br><a style="color:var(--warning)">요청 보기</a></span></div>'
                s += '<pre>' + e(_plain(TERM[name])) + '</pre></div>'
            return s + '</div>'
        ns['sidebar'], ns['workspace'] = sidebar, workspace
        ns['palettes'] = {theme: {**base[theme], **CODEX_VARIANTS[key][theme]}}
        doc = ns['document'](theme, 'workspace')
        for a, b in [('sample / task-000', 'pod / 터미널 넓은 글자 정렬'), ('pod/task-000', 'pod/wide-glyph'), ('</svg>49<', '</svg>3<'),
                     ('응답 50', '응답 2'), ('</svg>51<', '</svg>5<'), ('layout.rs</div>', 'terminal.rs</div>')]:
            doc = doc.replace(a, b)
        return doc.replace('</style>', '.grid{min-height:0;grid-template-rows:repeat(2,minmax(0,1fr))}.terminal{min-height:0;overflow:hidden}</style>')
    return fn


# --------------------------------------------------------------------------- catalogue
EXISTING = [
    dict(id='01', key='codex-a', src=('d4/dark-workspace.png', 'd4/light-workspace.png'), name='정밀한 작업대', origin='Codex A (권장)', cat='기존 6안',
         line='240px 트리 + 평평한 dock, 28px 작업자 행', density='중간 · 행 28/32', colour='차가운 남색 회색, 파랑 강조',
         pro='현재 구조·단축키·복원을 유지한 채 체감 품질을 올림', con='300 작업자 규모에서는 검색 의존'),
    dict(id='02', key='codex-b', src=('d4/b-dark-workspace.png', 'd4/b-light-workspace.png'), name='한 작업에 집중', origin='Codex B', cat='기존 6안',
         line='44px 레일 + 상단 작업 전환, 영구 트리 없음', density='낮음 · 중앙 폭 최대', colour='Codex A와 동일 토큰',
         pro='현재 작업 터미널 면적이 가장 큼', con='전체 승인 발견에 drawer 필요, 레이아웃 마이그레이션'),
    dict(id='03', key='codex-c', src=('d4/c-dark-workspace.png', 'd4/c-light-workspace.png'), name='응답 중심 관제', origin='Codex C', cat='기존 6안',
         line='280px 응답 queue + 집계 3개', density='중간 · 76px 요청 행', colour='Codex A와 동일 토큰',
         pro='많은 에이전트의 응답 병목을 바로 발견', con='코딩 집중에 불리, 넓은 왼쪽 영역'),
    dict(id='04', key='claude-a', src=('d4-claude/A-quiet/dark-workspace.png', 'd4-claude/A-quiet/light-workspace.png'), name='조용한 편집기', origin='Claude A (권장)', cat='기존 6안',
         line='평평한 면 3단계 + 1px 선, 높은 밀도', density='높음 · 행 28', colour='중립 회색, 차분한 파랑 #3D6FD6',
         pro='오래 봐도 피로가 적고 GPUI 구현 비용 최소', con='첫인상이 평범할 수 있음'),
    dict(id='05', key='claude-b', src=('d4-claude/B-layered/dark-workspace.png', 'd4-claude/B-layered/light-workspace.png'), name='레이어형', origin='Claude B', cat='기존 6안',
         line='바깥 틀 위 둥근 패널, 테두리 대신 면 단계', density='중간 · 행 30', colour='인디고 강조, 라이트에 그림자',
         pro='가장 부드럽고 현대적, 라이트가 고급스러움', con='간격·그림자만큼 터미널 면적 감소'),
    dict(id='06', key='claude-c', src=('d4-claude/C-mission/dark-workspace.png', 'd4-claude/C-mission/light-workspace.png'), name='관제형', origin='Claude C', cat='기존 6안',
         line='상태 큐 + 상태색 타일, 에이전트가 주인공', density='중간 · 큐 카드', colour='청록 강조, 상태색 면적 큼',
         pro='감독 도구라는 정체성이 가장 분명', con='화면이 시끄러워지기 쉽고 레이아웃 변경 큼'),
]
NEW = [
    dict(id='07', key='dense', fn=s_dense, name='고밀도 편집기', cat='레이아웃·밀도',
         line='22px 행, 단색 아이콘, 액티비티 바 + 출력 패널 + 세부 정보 열', density='매우 높음 · 행 22, UI 12px', colour='거의 무채색, 응답/실패만 색',
         pro='한 화면에 작업 6개·이벤트·세부 정보가 모두 보여 많은 작업자를 다루기 좋음', con='12px UI와 22px 행은 150% 이하·접근성 모드에서 답답함, 초심자에게 위계가 약함'),
    dict(id='08', key='islands', fn=s_islands, name='섬형', cat='표면·색',
         line='어두운 캔버스 위 둥근 패널, 10px 간격, 떠 있는 상태 섬', density='낮음 · 행 32', colour='인디고 강조, 캔버스와 섬의 명도차',
         pro='영역 구분이 즉각적이고 라이트에서 특히 정돈돼 보임', con='간격·반경만큼 터미널 면적이 약 6% 줄고 GPUI 반경 클리핑 비용'),
    dict(id='09', key='mono', fn=s_mono, name='미니멀 단색', cat='표면·색',
         line='경계선 없이 여백과 글자 굵기로 구분, 상태만 색', density='낮음 · 넓은 여백', colour='#0A0A0A / #FFFFFF, 강조색 없음',
         pro='가장 조용하고 타이포그래피가 돋보임, 상태색이 확실히 눈에 띔', con='선택·focus를 굵기와 막대에 의존해 발견성이 낮음, 많은 작업에선 길을 잃기 쉬움'),
    dict(id='10', key='warm', fn=s_warm, name='따뜻한 중성색', cat='표면·색',
         line='종이 같은 베이지 면, 큰 터미널 1 + 작은 3, 녹 갈색 강조', density='중간 · 행 32', colour='웜 그레이/베이지, 강조 #A4532B',
         pro='장시간 라이트 사용에 눈이 편하고 개성이 뚜렷함', con='코드·diff 색과 어울리는 웜 팔레트를 따로 맞춰야 하고 취향을 탐'),
    dict(id='11', key='blocks', fn=s_blocks, name='블록 터미널', cat='작업 흐름',
         line='에이전트 출력을 명령 블록으로 쌓은 시간순 피드, 승인은 블록 안 버튼', density='중간 · 블록 머리 30', colour='Claude A 기반 중립',
         pro='무엇을 실행했고 어디서 막혔는지 한눈에, 승인·접기·복사가 블록 단위', con='VT 출력에서 명령 경계를 안정적으로 잘라야 함(셸 통합/hook 필요), 전체 화면 TUI와 충돌'),
    dict(id='12', key='kanban', fn=s_kanban, name='칸반 보드', cat='작업 흐름',
         line='작업 중 / 응답 필요 / 완료 열, 끌 수 있는 작업자 카드 + 하단 터미널 서랍', density='낮음 · 카드', colour='청록 강조, 상태색 왼쪽 띠',
         pro='여러 작업의 진행 상황을 관리자 시점에서 가장 쉽게 파악', con='상태는 에이전트가 정하므로 끌기의 의미가 모호, 터미널이 부차적'),
    dict(id='13', key='review', fn=s_review, name='분할 리뷰 중심', cat='작업 흐름',
         line='나란히 diff가 주 캔버스, 터미널 4개는 하단에 작게 도킹', density='중간 · diff 20px 행', colour='Claude A 기반 중립',
         pro='에이전트 결과 검토·의견 전달이 주 업무일 때 최적', con='터미널 입력·관찰이 불편, 작업 중 화면과 전환 비용'),
    dict(id='14', key='access', fn=s_access, name='고대비·접근성 우선', cat='접근성',
         line='40px 목표, 15px 글자, 3px 노란/파란 focus, 상태 문구 상시 표시', density='낮음 · 행 36–40', colour='강한 경계, AA 이상 대비',
         pro='키보드·저시력·고DPI 사용자에게 가장 확실하고 상태가 색에 의존하지 않음', con='정보량이 약 35% 줄어 많은 작업자 감독에는 스크롤 증가'),
    dict(id='15', key='a-teal', fn=claude_a_variant('a-teal'), name='조용한 편집기 · 청록', origin='Claude A 변형', cat='색 변형',
         line='04와 같은 구조, 차가운 청회색 면 + 청록 강조', density='높음 · 행 28', colour='cool gray, 강조 #1F8A86',
         pro='파랑보다 차분하고 Pod 고유색으로 기억되기 쉬움', con='청록이 working(하늘색)·done(초록) 상태색과 가까워 구분 주의'),
    dict(id='16', key='a-violet', fn=claude_a_variant('a-violet'), name='조용한 편집기 · 보라', origin='Claude A 변형', cat='색 변형',
         line='04와 같은 구조, 순수 중립 회색 + 보라 강조', density='높음 · 행 28', colour='neutral gray, 강조 #7353D6',
         pro='상태색(파랑·주황·초록·빨강)과 겹치지 않아 강조가 분명', con='보라가 AI 제품에서 흔해 차별성은 낮음'),
    dict(id='17', key='codex-neutral', fn=codex_a_variant('codex-neutral'), name='정밀한 작업대 · 중립 회색', origin='Codex A 변형', cat='색 변형',
         line='01의 레이아웃 코드를 공통 fixture로 다시 렌더, 남색 기운을 뺀 순수 회색 면', density='중간 · 행 28/32', colour='neutral gray, 파랑 강조 유지',
         pro='터미널 ANSI 색이 가장 정확하게 보임', con='01보다 다소 밋밋하고 깊이감이 약함'),
    dict(id='18', key='codex-green', fn=codex_a_variant('codex-green'), name='정밀한 작업대 · 녹청', origin='Codex A 변형', cat='색 변형',
         line='01의 레이아웃 코드를 공통 fixture로 다시 렌더, 녹색 기운 면 + 녹청 강조', density='중간 · 행 28/32', colour='green-tinted gray, 강조 #2A7A63',
         pro='눈이 편하고 터미널 도구다운 인상', con='강조와 success 색이 비슷해 완료 상태가 덜 두드러짐'),
]


# --------------------------------------------------------------------------- contrast audit (WCAG 2.x, same formula as tools/visual/contrast.py)
def lum(h):
    c = [int(h[i:i + 2], 16) / 255 for i in (1, 3, 5)]
    c = [x / 12.92 if x <= 0.04045 else ((x + 0.055) / 1.055) ** 2.4 for x in c]
    return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]


def ratio(a, b):
    la, lb = sorted((lum(a), lum(b)), reverse=True)
    return (la + 0.05) / (lb + 0.05)


def audit():
    rows, worst = [], {}
    for key, themes in P.items():
        for theme, p in themes.items():
            pairs = [(f, b, 4.5) for f in ('text', 'text2', 'muted', 'working', 'attention', 'done', 'failed', 'accent_fg')
                     for b in ('app', 'sidebar', 'panel', 'selected', 'hover')]
            pairs += [('term_fg', 'term_bg', 4.5), ('focus', 'panel', 3), ('focus', 'app', 3)]
            for f, b, m in pairs:
                r = ratio(p[f], p[b])
                rows.append(dict(sample=key, theme=theme, fg=f, bg=b, ratio=round(r, 2), min=m, ok=r >= m))
            worst[f'{key}/{theme}'] = min(r['ratio'] for r in rows if r['sample'] == key and r['theme'] == theme and r['min'] == 4.5)
    return rows, worst


# --------------------------------------------------------------------------- render
def render(jobs):
    from playwright.sync_api import sync_playwright
    HTML.mkdir(parents=True, exist_ok=True)
    IMG.mkdir(parents=True, exist_ok=True)
    with sync_playwright() as p:
        try:
            browser = p.chromium.launch(channel='chrome')
        except Exception:
            browser = p.chromium.launch(executable_path='C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe')
        ctx = browser.new_context(viewport={'width': 960, 'height': 640}, device_scale_factor=1.5, reduced_motion='reduce', offline=True)
        pg = ctx.new_page()
        for s in jobs:
            for theme in ('dark', 'light'):
                stem = f'{s["id"]}-{s["key"]}-{theme}'
                h = HTML / f'{stem}.html'
                h.write_text(s['fn'](theme), encoding='utf-8')
                pg.goto(h.resolve().as_uri())
                pg.wait_for_timeout(120)
                pg.screenshot(path=str(IMG / f'{stem}.png'))
        browser.close()


def main():
    only = set(sys.argv[1:])
    rows, worst = audit()
    bad = [r for r in rows if not r['ok']]
    for k, v in worst.items():
        print(f'contrast {k}: min text {v}')
    for r in bad:
        print('FAIL', r)
    (HERE / 'contrast.json').write_text(json.dumps(rows, indent=1, ensure_ascii=False) + '\n', encoding='utf-8')
    render([s for s in NEW if not only or s['id'] in only])
    for s in EXISTING:  # copy the six existing directions unchanged
        for theme, rel in zip(('dark', 'light'), s['src']):
            shutil.copyfile(DESIGN / rel, IMG / f'{s["id"]}-{s["key"]}-{theme}.png')
    cards = []
    for s in EXISTING + NEW:
        cards.append({k: s.get(k, '') for k in ('id', 'key', 'name', 'origin', 'cat', 'line', 'density', 'colour', 'pro', 'con')}
                     | {'dark': f'images/{s["id"]}-{s["key"]}-dark.png', 'light': f'images/{s["id"]}-{s["key"]}-light.png',
                        'new': s in NEW})
    tpl = (HERE / 'index.template.html').read_text(encoding='utf-8')
    data = json.dumps(cards, ensure_ascii=False, indent=1).replace('</', '<\\/')
    (GAL / 'index.html').write_text(tpl.replace('/*DATA*/[]', data, 1), encoding='utf-8')
    write_md(cards, worst)
    total = sum(f.stat().st_size for f in IMG.glob('*.png'))
    big = [(f.name, f.stat().st_size) for f in IMG.glob('*.png') if f.stat().st_size > 400_000]
    print(f'images {len(list(IMG.glob("*.png")))} total {total / 1e6:.2f} MB; over 400KB: {big}')
    if bad:
        sys.exit(1)


def write_md(cards, worst):
    out = ['# Pod 디자인 샘플 모음', '',
           '2026-11-10 · 브랜치 `SJin-Choi/d4-samples` · 문서·목업만(앱 코드 변경 없음). 갤러리: [index.html](index.html)', '',
           '방향을 고르기 전에 비교해 보기 위한 샘플입니다. 01–06은 기존 D4 6안의 작업 공간 PNG를 그대로 가져왔고(01–03은 Codex가 쓴 `sample/task-000` fixture), 07–18은 새로 만들었습니다.',
           '모든 새 샘플은 같은 한국어 fixture(작업 6개, Claude/Codex 작업자, 승인 대기 `Codex · 리뷰`, 실패 `LSP 지연 시작 · Codex · 구현`)를',
           '같은 크기(960×640 논리 px @150% = 1440×960)로 다크와 라이트 두 테마로 렌더링했습니다.', '',
           '재현: 저장소 root에서 `python -I docs/design/gallery/src/build.py` (Playwright + 로컬 Chrome, 없으면 Edge. 네트워크 사용 안 함).',
           '아이콘은 D4-claude가 이미 반입한 Lucide(ISC) SVG를 재사용합니다. 외부 제품의 화면·자산은 쓰지 않았습니다.', '',
           '| # | 이름 | 분류 | 성격 | 밀도 · 색 | Pod에 좋은 점 | Pod에 걸리는 점 |', '|---|---|---|---|---|---|---|']
    for c in cards:
        nm = c['name'] + (f' <br><sub>{c["origin"]}</sub>' if c['origin'] else '')
        out.append(f'| {c["id"]} | {nm} | {c["cat"]} | {c["line"]} | {c["density"]} · {c["colour"]} | {c["pro"]} | {c["con"]} |')
    out += ['', '## 이미지', '', '`images/<번호>-<키>-<dark|light>.png`, 각 1440×960. 새 샘플의 HTML 원본은 `src/html/`에 있습니다.', '',
            '## 대비 검사(새 팔레트)', '',
            '`src/contrast.json`: 본문·보조·muted·상태·강조 글자를 app/sidebar/panel/selected/hover 면에 대해 4.5 이상, focus는 3 이상으로 검사했습니다.',
            'Codex A 변형(17, 18)은 같은 역할의 값만 바꿨고 원본 build.py의 검사 대상이 아니므로 아래 표에 없습니다. 목업 대비 검사는 실제 GPUI 합성 결과를 대신하지 않습니다.', '',
            '| 샘플/테마 | 최소 글자 대비 |', '|---|---|']
    out += [f'| {k} | {v} |' for k, v in worst.items()]
    out += ['', '## 고를 때 참고', '',
            '- 구조를 바꾸지 않는 선택: 01/04 및 색 변형 15–18 중 하나 → 바로 D4.9 Foundation 단계로 갈 수 있습니다.',
            '- 07(고밀도)과 14(접근성)는 별도 방향이라기보다 **밀도 설정의 양 끝**으로 함께 제공할 수 있습니다.',
            '- 11(블록), 12(칸반), 13(리뷰 중심)은 기본 화면보다는 **추가 보기 모드** 후보입니다. 새 기능 범위(명령 경계 인식, 보드 상태 모델, 리뷰 dock)가 필요합니다.',
            '- 08(섬), 09(단색), 10(웜)은 레이아웃보다 **표면 문법**의 선택이라 01/04 구조 위에 얹을 수 있습니다.', '']
    (GAL / 'samples.md').write_text('\n'.join(out), encoding='utf-8')


if __name__ == '__main__':
    main()
