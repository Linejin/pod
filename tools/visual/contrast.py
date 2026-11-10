"""Audit every built-in style theme (crates/pod-ui/themes/*.json, docs/10 §D5) against WCAG AA.

Same pairs and formula as tools/themes/gen.py and pod_ui::themes::contrast_pairs: text 4.5:1 on
canvas/chrome/surface/raised/hover/selected, status text 4.5:1, icons/borders/focus 3:1.
Disabled text and decorative dividers are excluded. Writes crates/pod-app/qa/d3/contrast.json.
"""
from pathlib import Path
import importlib.util
import json

root = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('pod_theme_gen', root / 'tools/themes/gen.py')
gen = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gen)

results = []
for path in sorted((root / 'crates/pod-ui/themes').glob('*.json')):
    style = json.loads(path.read_text(encoding='utf-8'))
    shared = json.loads((root / 'crates/pod-ui/themes/workbench.json').read_text(encoding='utf-8'))['colors']
    palettes = {p: style['colors'].get(p) or shared[p] for p in ('dark', 'light', 'hc-dark', 'hc-light')}
    for name, colors in palettes.items():
        variants = [(name, colors)]
        if style.get('accent_override') and not name.startswith('hc'):
            variants += [(f'{name}+{a}', {**colors, **v[name]}) for a, v in style.get('accents', {}).items()]
        for label, c in variants:
            for fg, bg, minimum in gen.pairs(c):
                r = gen.ratio(c[fg], c[bg])
                results.append(dict(style=style['id'], palette=label, foreground=fg, background=bg, ratio=round(r, 3), minimum=minimum, passed=r >= minimum))
target = root / 'crates/pod-app/qa/d3/contrast.json'
target.parent.mkdir(parents=True, exist_ok=True)
target.write_text(json.dumps(results, indent=1) + '\n', encoding='utf-8')
print(f"{sum(r['passed'] for r in results)}/{len(results)} AA pairs pass across {len({r['style'] for r in results})} styles")
for row in results:
    if not row['passed']:
        print(row)
raise SystemExit(0 if all(row['passed'] for row in results) else 1)
