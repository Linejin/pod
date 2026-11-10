"""Documentation-only, original Pod mockups. No network, no product modifications."""
from pathlib import Path
import json, runpy, contextlib, io, html
from PIL import Image, ImageDraw
from playwright.sync_api import sync_playwright

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
# Reuse the repository's actual WCAG implementation; also run its existing audit.
with contextlib.redirect_stdout(io.StringIO()) as log:
    try:
        audit = runpy.run_path(str(ROOT / 'tools/visual/contrast.py'))
    except SystemExit as e:
        assert e.code == 0
# run_path exits before returning globals, so load just its pure function section.
source = (ROOT / 'tools/visual/contrast.py').read_text(encoding='utf-8')
ns = {}
exec(source[source.index('def luminance'):source.index('results = []')], ns)
ratio = ns['ratio']
print(log.getvalue().strip())

names = 'app sidebar panel raised overlay hover pressed selected hairline control primary secondary muted disabled accent button button_hover button_pressed button_fg success success_bg warning warning_bg danger danger_bg info info_bg terminal terminal_fg cursor'.split()
values = {
 'dark': '17191D 1C1F24 202329 272B32 2B3038 303640 353D48 293B50 363D47 8793A3 E9EDF2 C2CAD4 ADB8C6 737D8B 9CC5FF 356CB1 2D609F 27558E FFFFFF 88C9AD 233B32 E2BE83 3E3425 EBA0AC 422B32 9CC5FF 293B50 191C20 E1E7EF BBD6FF',
 'light': 'F5F6F8 EEF0F3 FFFFFF F8F9FB FFFFFF E5E9EF DCE2EA E0EAF6 D8DDE5 748195 202630 465264 536174 8A94A3 245B9B 285F9F 21538D 1C477A FFFFFF 21684E EAF4EE 79500D FBF1DF A23449 F9ECEF 245B9B E0EAF6 FCFCFD 283343 245B9B',
 'hc-dark': '000000 000000 000000 000000 000000 172536 172536 172536 FFFFFF FFFFFF FFFFFF FFFFFF FFFFFF A0A0A0 FFFF00 FFFF00 FFFF00 FFFF00 000000 80FFB0 000000 FFFF80 000000 FFAABB 000000 AADDFF 000000 000000 FFFFFF FFFF00',
 'hc-light': 'FFFFFF FFFFFF FFFFFF FFFFFF FFFFFF E5EDF6 E5EDF6 E5EDF6 000000 000000 000000 000000 000000 666666 003E80 003E80 003E80 003E80 FFFFFF 00552B FFFFFF 684000 FFFFFF 8A1631 FFFFFF 003E80 FFFFFF FFFFFF 000000 003E80'
}
palettes = {mode: dict(zip(names, ['#'+v for v in row.split()])) for mode,row in values.items()}
(HERE/'tokens.json').write_text(json.dumps(palettes,indent=2)+'\n',encoding='utf-8')
results=[]
for mode,t in palettes.items():
    surfaces='app sidebar panel raised overlay hover pressed selected'.split()
    pairs=[(fg,bg,4.5) for fg in ['primary','secondary','muted','accent','success','warning','danger','info'] for bg in surfaces]
    pairs += [(fg,bg,3) for fg in ['control','accent'] for bg in surfaces]
    pairs += [('button_fg',bg,4.5) for bg in ['button','button_hover','button_pressed']]
    pairs += [(s,s+'_bg',4.5) for s in ['success','warning','danger','info']]
    pairs += [('terminal_fg','terminal',4.5),('cursor','terminal',3)]
    for fg,bg,minimum in pairs:
        r=ratio(int(t[fg][1:],16),int(t[bg][1:],16))
        results.append(dict(theme=mode,foreground=fg,background=bg,ratio=round(r,3),minimum=minimum,passed=r>=minimum))
(HERE/'contrast.json').write_text(json.dumps(results,indent=2)+'\n',encoding='utf-8')
for mode in palettes:
    rows=[r for r in results if r['theme']==mode]
    print(mode,sum(r['passed'] for r in rows),'/',len(rows),'min text',min(r['ratio'] for r in rows if r['minimum']==4.5))
bad=[r for r in results if not r['passed']]
assert not bad,bad

# Original schematic strokes: intentionally not copied Lucide assets. Production uses Lucide.
paths={
 'search':'<circle cx="10" cy="10" r="6"/><path d="m15 15 5 5"/>',
 'plus':'<path d="M12 5v14M5 12h14"/>',
 'more':'<circle cx="5" cy="12" r="1"/><circle cx="12" cy="12" r="1"/><circle cx="19" cy="12" r="1"/>',
 'chev':'<path d="m9 5 7 7-7 7"/>',
 'down':'<path d="m5 9 7 7 7-7"/>',
 'terminal':'<path d="m5 7 5 5-5 5M13 17h6"/>',
 'grid':'<rect x="4" y="4" width="16" height="16" rx="2"/><path d="M12 4v16M4 12h16"/>',
 'panel':'<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M9 4v16"/>',
 'branch':'<circle cx="6" cy="5" r="2"/><circle cx="6" cy="19" r="2"/><circle cx="18" cy="5" r="2"/><path d="M6 7v10M18 7c0 7-12 3-12 9"/>',
 'bell':'<path d="M5 17h14l-2-3V9a5 5 0 0 0-10 0v5zM10 21h4"/>',
 'settings':'<circle cx="12" cy="12" r="4"/><path d="M12 2v3M12 19v3M2 12h3M19 12h3M5 5l2 2M17 17l2 2M5 19l2-2M17 7l2-2"/>',
 'close':'<path d="m6 6 12 12M6 18 18 6"/>',
 'work':'<path d="M20 12a8 8 0 1 1-8-8"/><path d="M16 4h4v4"/>',
 'wait':'<path d="m12 3 8 4v5c0 5-8 9-8 9s-8-4-8-9V7zM12 8v5M12 16v1"/>',
 'done':'<circle cx="12" cy="12" r="9"/><path d="m7 12 3 3 7-7"/>',
 'idle':'<circle cx="12" cy="12" r="8"/>',
 'error':'<path d="m8 3-5 5v8l5 5h8l5-5V8l-5-5zM12 7v6M12 16v1"/>',
 'file':'<path d="M5 3h9l5 5v13H5zM14 3v6h5M8 13h8M8 17h6"/>',
 'up':'<path d="M12 20V4m-6 6 6-6 6 6"/>',
 'arrowdown':'<path d="M12 4v16m-6-6 6 6 6-6"/>',
 'refresh':'<path d="M20 10a8 8 0 1 0 0 5M20 3v7h-7"/>',
 'comment':'<path d="M3 4h18v13H9l-6 4z"/>',
 'send':'<path d="m3 3 18 9-18 9 4-9zM7 12h14"/>',
 'tree':'<path d="M5 3v15h7M5 8h7"/><rect x="12" y="5" width="8" height="6" rx="1"/><rect x="12" y="15" width="8" height="6" rx="1"/>',
 'minus':'<path d="M5 12h14"/>',
 'square':'<rect x="5" y="5" width="14" height="14" rx="1"/>'
}
def icon(name,cls=''):
    return f'<svg class="icon {cls}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">{paths[name]}</svg>'
def btn(name,label=''):
    return f'<button title="{label}">{icon(name)}{label if False else ""}</button>'
def row(name,state='idle',selected=False,task=False,meta=''):
    color={'work':'accent','wait':'warning','done':'success','idle':'muted','error':'danger'}[state]
    return f'<div class="row {"task" if task else "worker"} {"selected" if selected else ""}">{icon("down") if task else ""}{icon(state,color)}<span class="name">{name}</span><small>{meta}</small></div>'
def sidebar():
    s='<aside><div class="sidehead"><b>작업 공간</b><span class="grow"></span>'+btn('plus','새 작업')+btn('terminal','새 터미널')+btn('more','더 보기')+'</div><div class="filter">'+icon('search')+'<span>작업, 터미널 검색</span></div><div class="section">sample <small>100</small><span class="grow"></span>'+btn('down')+'</div>'
    titles=['한글 작업과 레이아웃','연결 복원 개선','리뷰 코멘트 정리']
    for i,title in enumerate(titles):
        s+=row(title,'wait' if i==0 else 'work',task=True,meta=str(i+1).zfill(2))
        for j in range(3):
            state=['work','wait','done'][j] if i<2 else ['idle','done','idle'][j]
            s+=row(f'Worker {i*3+j}',state,selected=(i==0 and j==0),meta='작업 중' if i==0 and j==0 else '')
    return s+'<div class="sidefoot">'+icon('tree')+'에이전트<span class="grow"></span>'+btn('more')+'</div></aside>'

css='''*{box-sizing:border-box}body{margin:0;font:13px/20px 'Segoe UI','Malgun Gothic',sans-serif;color:var(--primary);background:var(--app)}button{font:inherit;color:var(--secondary);background:transparent;border:0;border-radius:4px;width:28px;height:28px;display:inline-flex;align-items:center;justify-content:center;padding:6px;flex-shrink:0}button:hover{background:var(--hover)}.icon{width:16px;height:16px;flex-shrink:0}.accent{color:var(--accent)}.warning{color:var(--warning)}.success{color:var(--success)}.danger{color:var(--danger)}.muted,small{color:var(--muted)}small{font-size:12px;line-height:16px}.grow{flex:1}.title{height:36px;display:flex;align-items:center;padding:0 10px;gap:12px;border-bottom:1px solid var(--hairline);background:var(--sidebar)}.title b{font-size:13px}.context{font-size:12px;color:var(--muted)}.command{display:flex;align-items:center;gap:8px;height:26px;width:232px;padding:0 8px;border:1px solid var(--hairline);background:var(--panel);border-radius:6px;color:var(--muted);font-size:12px}.command kbd{margin-left:auto;font:11px 'Segoe UI';color:var(--muted)}.caption{display:flex;gap:4px;margin-left:8px}.body{display:flex;height:580px}aside{width:240px;flex-shrink:0;background:var(--sidebar);border-right:1px solid var(--hairline);display:flex;flex-direction:column;overflow:hidden}.sidehead{height:36px;display:flex;align-items:center;padding:0 8px;flex-shrink:0}.filter{margin:4px 8px 8px;border:1px solid var(--control);height:28px;border-radius:4px;display:flex;align-items:center;padding:0 8px;gap:8px;color:var(--muted);font-size:12px;flex-shrink:0}.section{height:28px;display:flex;align-items:center;gap:8px;padding:0 12px;color:var(--secondary);font-weight:600}.row{height:28px;min-height:28px;display:flex;align-items:center;gap:8px;padding:0 12px;border-left:2px solid transparent}.row .name{white-space:nowrap;overflow:hidden;text-overflow:ellipsis;flex:1}.row.task{font-weight:600;height:32px;min-height:32px;margin-top:4px;gap:4px;font-size:13px}.task .icon:first-child{width:12px}.task .icon:nth-child(2){width:12px}.worker{padding-left:36px;color:var(--secondary)}.selected{background:var(--selected);border-left-color:var(--accent);color:var(--primary)}.sidefoot{margin-top:auto;display:flex;align-items:center;gap:8px;padding:4px 12px;border-top:1px solid var(--hairline);background:var(--sidebar)}main{flex:1;min-width:0;background:var(--panel);display:flex;flex-direction:column}.tabs{height:32px;flex-shrink:0;background:var(--sidebar);border-bottom:1px solid var(--hairline);display:flex;align-items:stretch}.tab{display:flex;align-items:center;gap:8px;padding:0 12px;font-size:12px;color:var(--muted);border-right:1px solid var(--hairline)}.tab.active{background:var(--panel);color:var(--primary);border-top:2px solid var(--accent)}.bar{height:36px;flex-shrink:0;display:flex;align-items:center;gap:8px;padding:0 12px;border-bottom:1px solid var(--hairline)}.grid{flex:1;display:grid;grid-template-columns:1fr 1fr;grid-template-rows:1fr 1fr;gap:1px;background:var(--hairline)}.terminal{background:var(--terminal);min-width:0}.panelhead{height:28px;display:flex;align-items:center;gap:8px;padding:0 10px;background:var(--panel);font-size:12px;border-bottom:1px solid var(--hairline)}pre{white-space:pre-wrap;font:12px/20px Consolas,'Malgun Gothic',monospace;margin:14px;color:var(--terminal_fg)}.banner{margin:12px;padding:10px 12px;display:flex;align-items:flex-start;gap:8px;background:var(--warning_bg);color:var(--primary);border-radius:4px;font-size:12px;line-height:20px}.banner .icon{margin-top:2px;color:var(--warning)}.action{width:auto;padding:0 12px;background:var(--button);color:var(--button_fg);height:28px;white-space:nowrap}.status{height:24px;display:flex;align-items:center;gap:10px;padding:0 10px;border-top:1px solid var(--hairline);background:var(--sidebar);font-size:12px;color:var(--secondary)}.status>span{display:flex;align-items:center;gap:5px}.status .icon{width:14px;height:14px}.sep{height:12px;border-left:1px solid var(--hairline);margin:0 2px}.seg{background:var(--sidebar);border:1px solid var(--control);border-radius:5px;display:flex;height:28px;padding:2px;align-items:center;font-size:12px}.seg span{padding:0 7px;line-height:22px;color:var(--muted)}.seg .on{background:var(--panel);color:var(--primary);border-radius:3px;box-shadow:0 1px 2px #0002}.reviewbody{display:flex;flex:1;min-height:0}.diff{flex:1;min-width:0}.filehead{height:32px;padding:6px 12px;display:flex;align-items:center;gap:8px;font-size:12px;background:var(--raised);border-bottom:1px solid var(--hairline)}.code{font:12px/22px Consolas,monospace;padding-top:12px}.line{display:flex;padding:0 12px;gap:16px;white-space:pre}.ln{color:var(--muted);width:22px;text-align:right;flex-shrink:0}.added{background:var(--success_bg)}.removed{background:var(--danger_bg)}.comments{width:224px;border-left:1px solid var(--hairline);padding:12px;display:flex;flex-direction:column;gap:12px;background:var(--panel);font-size:12px;line-height:20px;overflow-wrap:anywhere}.comment{padding:12px;background:var(--raised);border:1px solid var(--hairline);border-radius:6px}.comment p{margin:8px 0}.field{border:1px solid var(--control);border-radius:4px;padding:8px;color:var(--muted);min-height:64px}.commentfoot{margin-top:auto;display:flex;flex-direction:column;gap:8px}.agenthead,.agentrow{display:grid;grid-template-columns:minmax(180px,1fr) 110px 112px 64px;gap:12px;padding:0 16px;align-items:center;height:32px;border-bottom:1px solid var(--hairline)}.agenthead{font-size:12px;color:var(--muted);background:var(--raised)}.agentrow{border-bottom:0}.agentrow>span:first-child{display:flex;gap:8px;align-items:center;min-width:0}.agentrow.child>span:first-child{padding-left:24px}.agentrow.child{color:var(--secondary)}.agentrow.group{margin-top:8px;font-weight:600}.agentrow.selected{border-left:2px solid var(--accent);padding-left:14px}.agentrow small{display:flex;align-items:center;gap:6px}.agentrow .label{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.agentdetail{margin:auto 16px 16px;padding:16px;border-top:1px solid var(--hairline);font-size:12px;color:var(--muted)}.empty{margin:auto;text-align:center;max-width:340px}.empty h2{font-size:18px;line-height:26px;font-weight:600}.empty p{color:var(--muted);font-size:13px}.errorbanner{background:var(--danger_bg)}.errorbanner .icon{color:var(--danger)}'''

def workspace():
    texts=[('Worker 0','work','PS sample/task-000> cargo test\n\n   Compiling pod-ui v0.1.0\n    Finished test profile in 1.24s\n\nrunning 24 tests\ntest tokens::contrast ... ok\ntest tree::navigation ... ok\n\n검증 결과를 정리하고 있습니다.'),('Worker 1','wait','승인 요청을 기다리고 있습니다.\n\n대상: src/layout.rs\n작업: 레이아웃 변경 사항 적용'),('Worker 2','done','PS sample/task-000> cargo check\n\n    Finished dev profile in 0.86s\n\n검사가 완료되었습니다.\nPS sample/task-000>'),('PowerShell','idle','PS sample/task-000> git status --short\n\n M src/layout.rs\n M src/theme.rs\n\nPS sample/task-000>')]
    s='<div class="bar"><b>한글 작업과 레이아웃</b><span class="grow"></span><small>pod/task-000</small>'+btn('grid','격자')+btn('more')+'</div><div class="grid">'
    for name,state,text in texts:
        s+='<div class="terminal"><div class="panelhead">'+icon(state,{'work':'accent','wait':'warning','done':'success','idle':'muted'}[state])+name+'<span class="grow"></span>'+btn('more')+'</div>'
        if state=='wait':s+='<div class="banner">'+icon('wait')+'<span>승인이 필요합니다<br><a style="color:var(--warning)">요청 보기</a></span></div>'
        s+='<pre>'+html.escape(text)+'</pre></div>'
    return s+'</div>'
def review(error=False):
    s='<div class="bar"><small>파일 3개</small><small class="success">+24</small><small class="danger">−8</small><span class="grow"></span><div class="seg"><span>나란히</span><span class="on">한 줄로</span></div>'+btn('up','이전 변경')+btn('arrowdown','다음 변경')+'<i class="sep"></i>'+btn('panel','파일 목록')+btn('comment','의견')+btn('more','더 보기')+'</div><div class="reviewbody"><div class="diff">'
    if error:s+='<div class="banner errorbanner">'+icon('error')+'<span><b>변경 사항을 불러오지 못했습니다</b><br>작업 폴더를 찾을 수 없습니다.<br>경로를 확인한 뒤 다시 시도하세요.</span><span class="grow"></span>'+btn('refresh','다시 시도')+'</div>'
    else:
        s+='<div class="filehead">'+icon('file')+'src/layout.rs<span class="grow"></span><small>1 / 3</small></div><div class="code">'
        lines=[('','  impl Layout {'),('','    fn row_height(&self) -> f32 {'),('removed','-     40.0'),('added','+     match self.density {'),('added','+       Density::Compact => 28.0,'),('added','+       Density::Comfortable => 36.0,'),('added','+     }'),('','    }'),('','  }')]
        for n,(cls,txt) in enumerate(lines):s+=f'<div class="line {cls}"><span class="ln">{24+n}</span>{html.escape(txt)}</div>'
        s+='</div>'
    s+='</div><div class="comments"><div><b>의견</b> <small>미해결 1</small></div><div class="comment"><small>src/layout.rs · 27줄</small><p>한글 작업 이름이 긴 경우에도 행 높이와 아이콘 정렬을 유지해 주세요.</p><small>검토자 · 방금</small></div><div class="commentfoot"><b>에이전트에 보내기</b><small>Worker 0 · 작업 중</small><div class="field">추가 요청 (선택)</div><button class="action">미해결 의견 1개 보내기</button></div></div></div>'
    return s
def agents():
    s='<div class="bar"><b>에이전트</b><small>실행 300</small><span class="grow"></span>'+btn('search')+btn('more')+'</div><div class="agenthead"><span>이름</span><span>프로그램</span><span>상태</span><span>최근 활동</span></div>'
    for i in range(3):
        s+=f'<div class="agentrow group"><span>{icon("down")}{icon("branch")}<span class="label">sample/task-{i:03}</span></span><small>작업 {i+1}</small><small class="warning">{icon("wait")}응답 1</small><small>방금</small></div>'
        for j,(state,label) in enumerate([('work','작업 중'),('wait','승인 필요'),('done','완료')]):
            color=['accent','warning','success'][j]
            s+=f'<div class="agentrow child {"selected" if i==0 and j==0 else ""}"><span>{icon("terminal")}<span class="label">Worker {i*3+j}</span></span><small>powershell</small><small class="{color}">{icon(state)}{label}</small><small>{j+1}초 전</small></div>'
    return s+'<div class="agentdetail"><b style="color:var(--primary)">Worker 0</b><br>sample/task-000 · powershell.exe<br>상태 출처: hook · 마지막 확인: 1초 전</div>'
def document(mode,scene):
    t=palettes[mode]; style=':root{'+''.join('--'+k+':'+v+';' for k,v in t.items())+'}'+css
    title='<header class="title"><b>Pod</b><span class="context">sample / task-000</span><span class="grow"></span><div class="command">'+icon('search')+'명령·터미널 검색<kbd>Ctrl K</kbd></div>'+btn('settings','설정')+'<div class="caption">'+btn('minus')+btn('square')+btn('close')+'</div></header>'
    tabnames={'workspace':'터미널','review':'리뷰','agents':'에이전트','review-error':'리뷰','workspace-empty':'터미널'}
    tabs='<div class="tabs"><div class="tab active">'+icon('terminal' if scene.startswith('workspace') else 'file' if scene.startswith('review') else 'tree')+tabnames[scene]+btn('close','닫기')+'</div><div class="tab">'+icon('file')+'layout.rs</div><span class="grow"></span>'+btn('more','패널 메뉴')+'</div>'
    if scene=='workspace':body=workspace()
    elif scene=='workspace-empty':body='<div class="empty">'+icon('terminal','muted')+'<h2>작업을 이어서 시작하세요</h2><p>왼쪽에서 터미널을 선택하거나<br>이 저장소에 새 작업을 만드세요.</p><button class="action">새 작업</button><p>Ctrl+K로 작업 찾기</p></div>'
    elif scene.startswith('review'):body=review(scene.endswith('error'))
    else:body=agents()
    status='<footer class="status"><span>'+icon('done','success')+'연결됨</span><i class="sep"></i><span>'+icon('work','accent')+'49</span><span>'+icon('wait','warning')+'응답 50</span><span>'+icon('done','success')+'51</span><span class="grow"></span><span>'+icon('branch')+'pod/task-000</span><i class="sep"></i><span>상태 '+icon('panel')+'</span><span>'+icon('bell')+'1</span></footer>'
    return '<!doctype html><html lang="ko"><meta charset="utf-8"><title>Pod D4 '+scene+' '+mode+'</title><style>'+style+'</style>'+title+'<div class="body">'+sidebar()+'<main>'+tabs+body+'</main></div>'+status+'</html>'

def direction(mode,scene,variant):
    doc=document(mode,scene)
    if variant=='b':
        rail='<nav class="rail">'+btn('panel','작업 목록')+btn('terminal','터미널')+btn('file','리뷰')+btn('tree','에이전트')+'<span class="grow"></span>'+btn('bell','응답 50')+'</nav>'
        doc=doc.replace(sidebar(),rail)
        strip='<div class="taskstrip"><span class="current">한글 작업과 레이아웃</span><span>연결 복원 개선</span><span>리뷰 코멘트 정리</span>'+btn('plus','새 작업')+'<span class="grow"></span><small>작업 전환</small>'+btn('down')+'</div>'
        doc=doc.replace('<main>','<main>'+strip)
        extra='.rail{width:44px;display:flex;flex-direction:column;align-items:center;padding:8px 0;gap:12px;background:var(--sidebar);border-right:1px solid var(--hairline)}.taskstrip{height:36px;flex-shrink:0;display:flex;align-items:center;gap:16px;padding:0 12px;background:var(--app);font-size:12px;color:var(--muted);border-bottom:1px solid var(--hairline)}.current{color:var(--primary);font-weight:600}.grid{grid-template-columns:2fr 1fr;grid-template-rows:1fr 1fr 1fr}.grid .terminal:first-child{grid-row:1/4}.comments{width:256px}.body{height:580px}'
    else:
        queue='<aside class="queue"><div class="sidehead"><b>작업 관제</b><span class="grow"></span>'+btn('plus')+btn('more')+'</div><div class="queuecounts"><div><b>49</b><small>작업 중</small></div><div><b class="warning">50</b><small>응답 필요</small></div><div><b>51</b><small>완료</small></div></div><div class="filter">'+icon('search')+'작업과 요청 검색</div><div class="section">응답 필요<span class="grow"></span><small>50</small></div>'
        for i,title in enumerate(['한글 작업과 레이아웃','연결 복원 개선','리뷰 코멘트 정리','터미널 입력 검사','언어 서버 재연결','키보드 탐색 수정']):
            queue+=f'<div class="queueitem {"selected" if i==0 else ""}"><div>{icon("wait","warning")}<b>{title}</b><span class="grow"></span><small>{i+1}분</small></div><p>Worker {i*3+1} · 승인 필요</p><small>sample / task-{i:03}</small></div>'
        queue+='<div class="sidefoot">'+icon('tree')+'전체 작업<span class="grow"></span><small>100</small></div></aside>'
        doc=doc.replace(sidebar(),queue)
        extra='.queue{width:280px}.queuecounts{display:flex;gap:24px;padding:12px 16px 16px}.queuecounts>div{display:flex;flex-direction:column;gap:4px}.queuecounts b{font-size:20px;line-height:24px;font-weight:600}.queueitem{padding:12px 16px;border-left:2px solid transparent;border-bottom:1px solid var(--hairline)}.queueitem.selected{border-left-color:var(--accent)}.queueitem>div{display:flex;gap:8px;align-items:center}.queueitem b{font-size:12px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.queueitem p{margin:4px 0 0 24px;font-size:12px;color:var(--secondary)}.queueitem>small{margin-left:24px}.comments{width:224px}.agenthead,.agentrow{grid-template-columns:minmax(150px,1fr) 96px 104px 56px;gap:8px}.grid pre{font-size:11px}'
    extra += '.grid{min-height:0;grid-template-rows:repeat(2,minmax(0,1fr))}.terminal{min-height:0;overflow:auto}'
    if variant=='b':
        extra += '.grid{grid-template-rows:repeat(3,minmax(0,1fr))}'
    return doc.replace('</style>',extra+'</style>')

with sync_playwright() as p:
    browser=p.chromium.launch(executable_path='C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe',headless=True)
    page=browser.new_page(viewport={'width':960,'height':640},device_scale_factor=1.5)
    for mode in ['dark','light']:
        for scene in ['workspace','review','agents','review-error','workspace-empty']:
            path=HERE/f'{mode}-{scene}.html';path.write_text(document(mode,scene),encoding='utf-8')
            page.goto(path.as_uri());page.screenshot(path=str(path.with_suffix('.png')))
        for variant in ['b','c']:
            for scene in ['workspace','review','agents']:
                path=HERE/f'{variant}-{mode}-{scene}.html';path.write_text(direction(mode,scene,variant),encoding='utf-8')
                page.goto(path.as_uri());page.screenshot(path=str(path.with_suffix('.png')))
        for scene in ['workspace','review','agents']:
            sheet=Image.new('RGB',(2880,1000),'#808080');d=ImageDraw.Draw(sheet)
            d.text((16,12),'BEFORE - shipped fixture / 144 DPI',fill='white')
            d.text((1456,12),'AFTER - D4 design proposal / 144 DPI (synthetic content)',fill='white')
            sheet.paste(Image.open(ROOT/f'tools/visual/baseline/{mode}-{scene}.png'),(0,40))
            sheet.paste(Image.open(HERE/f'{mode}-{scene}.png'),(1440,40))
            sheet.save(HERE/f'{mode}-{scene}-before-after.png')
    browser.close()
