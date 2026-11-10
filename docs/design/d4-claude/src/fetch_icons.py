"""Fetch the pinned icon SVGs used by the mockups (run once; results are committed).
Lucide 0.468.0 (ISC; Feather-derived MIT), Fluent UI System Icons 1.1.345 (MIT), Phosphor core 2.1.1 (MIT)."""
import pathlib, urllib.request, sys
HERE = pathlib.Path(__file__).parent / 'icons'
# logical name -> (lucide, fluent, phosphor)
MAP = {
 'search': ('search', 'search_16_regular', 'regular/magnifying-glass'),
 'plus': ('plus', 'add_16_regular', 'regular/plus'),
 'terminal': ('square-terminal', 'window_console_20_regular', 'regular/terminal-window'),
 'grid': ('layout-grid', 'grid_16_regular', 'regular/squares-four'),
 'bot': ('bot', 'bot_20_regular', 'regular/robot'),
 'settings': ('settings', 'settings_16_regular', 'regular/gear-six'),
 'bell': ('bell', 'alert_16_regular', 'regular/bell'),
 'panel-left': ('panel-left', 'panel_left_16_regular', 'regular/sidebar-simple'),
 'panel-right': ('panel-right', 'panel_right_20_regular', 'regular/sidebar'),
 'chevron-right': ('chevron-right', 'chevron_right_16_regular', 'regular/caret-right'),
 'chevron-down': ('chevron-down', 'chevron_down_16_regular', 'regular/caret-down'),
 'chevron-left': ('chevron-left', 'chevron_left_16_regular', 'regular/caret-left'),
 'ellipsis': ('ellipsis', 'more_horizontal_16_regular', 'regular/dots-three'),
 'x': ('x', 'dismiss_16_regular', 'regular/x'),
 'minus': ('minus', 'subtract_16_regular', 'regular/minus'),
 'square': ('square', 'square_16_regular', 'regular/square'),
 'folder-git': ('folder-git-2', 'folder_16_regular', 'regular/folder-simple'),
 'git-branch': ('git-branch', 'branch_16_regular', 'regular/git-branch'),
 'git-pr': ('git-pull-request', 'branch_request_16_regular', 'regular/git-pull-request'),
 'working': ('loader-circle', 'arrow_sync_circle_16_regular', 'regular/circle-notch'),
 'approval': ('shield-question', 'shield_question_16_regular', 'fill/shield-warning-fill'),
 'question': ('message-circle-question', 'chat_help_20_regular', 'fill/chat-circle-dots-fill'),
 'done': ('circle-check', 'checkmark_circle_16_regular', 'fill/check-circle-fill'),
 'idle': ('circle', 'circle_16_regular', 'regular/circle'),
 'unknown': ('circle-help', 'question_circle_16_regular', 'regular/question'),
 'alert': ('triangle-alert', 'warning_16_regular', 'fill/warning-fill'),
 'failed': ('octagon-x', 'dismiss_circle_16_regular', 'fill/x-circle-fill'),
 'comment': ('message-square', 'comment_16_regular', 'regular/chat-centered'),
 'arrow-up': ('arrow-up', 'arrow_up_16_regular', 'regular/arrow-up'),
 'arrow-down': ('arrow-down', 'arrow_down_16_regular', 'regular/arrow-down'),
 'refresh': ('rotate-cw', 'arrow_clockwise_16_regular', 'regular/arrow-clockwise'),
 'split': ('columns-2', 'split_vertical_16_regular', 'regular/columns'),
 'unified': ('rows-2', 'split_horizontal_16_regular', 'regular/rows'),
 'wrap': ('wrap-text', 'text_wrap_16_regular', 'regular/text-align-left'),
 'expand': ('unfold-vertical', 'arrow_expand_16_regular', 'regular/arrows-out-line-vertical'),
 'file-diff': ('file-diff', 'document_16_regular', 'regular/file-text'),
 'send': ('send', 'send_16_regular', 'regular/paper-plane-right'),
 'check': ('check', 'checkmark_16_regular', 'regular/check'),
 'plug': ('plug', 'plug_connected_20_regular', 'regular/plugs-connected'),
 'command': ('command', 'keyboard_16_regular', 'regular/command'),
 'layers': ('layers', 'layer_20_regular', 'regular/stack'),
 'maximize': ('maximize-2', 'full_screen_maximize_16_regular', 'regular/arrows-out'),
 'filter': ('list-filter', 'filter_16_regular', 'regular/funnel-simple'),
 'eye': ('eye', 'eye_16_regular', 'regular/eye'),
}
URLS = {
 'lucide': 'https://unpkg.com/lucide-static@0.468.0/icons/{}.svg',
 'fluent': 'https://unpkg.com/@fluentui/svg-icons@1.1.345/icons/{}.svg',
 'phosphor': 'https://unpkg.com/@phosphor-icons/core@2.1.1/assets/{}.svg',
}
bad = []
for logical, names in MAP.items():
    for fam, name in zip(URLS, names):
        out = HERE / fam / f'{logical}.svg'
        if out.exists():
            continue
        out.parent.mkdir(parents=True, exist_ok=True)
        try:
            data = urllib.request.urlopen(URLS[fam].format(name), timeout=20).read()
            if b'<svg' not in data[:400]:
                raise ValueError('not svg')
            out.write_bytes(data)
        except Exception as e:
            bad.append((fam, logical, name, str(e)[:60]))
for b in bad:
    print('MISSING', *b)
print('done', len(bad), 'missing')
