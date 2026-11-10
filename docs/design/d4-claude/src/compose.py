"""Before (tools/visual/baseline, 150%) / after (A mockup, same 1440x960) side by side, plus a direction overview."""
import pathlib
from PIL import Image, ImageDraw, ImageFont
HERE = pathlib.Path(__file__).parent
OUT = HERE.parent
ROOT = HERE.parents[3]
font = ImageFont.truetype('C:/Windows/Fonts/malgunbd.ttf', 28)

def label(img, text):
    d = ImageDraw.Draw(img)
    y = img.height - 44
    d.rectangle([img.width - 24 - 17 * len(text), y, img.width, img.height], fill=(0, 0, 0))
    d.text((img.width - 14 - 17 * len(text), y + 6), text, fill=(255, 255, 255), font=font)

def pair(before, after, out, scale=0.5):
    a, b = Image.open(before).convert('RGB'), Image.open(after).convert('RGB')
    w, h = int(a.width * scale), int(a.height * scale)
    canvas = Image.new('RGB', (w * 2 + 12, h), (128, 128, 128))
    for i, (img, t) in enumerate([(a, '현재 (before)'), (b, 'D4-A (after)')]):
        im = img.resize((w, h), Image.LANCZOS); label(im, t); canvas.paste(im, (i * (w + 12), 0))
    canvas.save(out, optimize=True)

(OUT / 'before-after').mkdir(exist_ok=True)
for theme in ('dark', 'light'):
    for screen in ('workspace', 'review', 'agents'):
        pair(ROOT / f'tools/visual/baseline/{theme}-{screen}.png', OUT / f'A-quiet/{theme}-{screen}.png',
             OUT / f'before-after/{theme}-{screen}.png')

# overview: 3 directions x dark/light
cells = [(f'{d}/{t}-workspace.png', f'{d[0]} {t}') for d in ('A-quiet', 'B-layered', 'C-mission') for t in ('dark', 'light')]
w, h = 720, 480
sheet = Image.new('RGB', (w * 2 + 12, h * 3 + 24), (128, 128, 128))
for i, (path, t) in enumerate(cells):
    im = Image.open(OUT / path).convert('RGB').resize((w, h), Image.LANCZOS); label(im, t)
    sheet.paste(im, ((i % 2) * (w + 12), (i // 2) * (h + 12)))
sheet.save(OUT / 'overview.png', optimize=True)
print('ok')
