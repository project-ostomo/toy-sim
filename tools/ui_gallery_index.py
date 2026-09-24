from pathlib import Path
import html
import os
import re

root = Path(__file__).resolve().parents[1]
gallery = Path(os.environ.get('OSG_UI_GALLERY', root / 'target/ui-gallery'))
plan = (root / 'docs/client-ui-implementation.md').read_text()
sections = []
missing = []
count = 0
for line in plan.splitlines():
    if not line.startswith('| ') or '`' not in line:
        continue
    title = line.split('|')[1].strip()
    cards = []
    for prefix in re.findall(r'`([^`]+)`', line):
        for size in ('1600x900', '900x650'):
            name = f'{prefix}-{size}.png'
            if not (gallery / name).exists():
                missing.append(name)
                continue
            count += 1
            safe = html.escape(name, quote=True)
            cards.append(f'<a href="{safe}"><img loading="lazy" src="{safe}" alt="{safe}"><span>{safe}</span></a>')
    sections.append(f'<section><h2>{html.escape(title)}</h2><div class="cards">{"".join(cards)}</div></section>')
assert not missing, missing
document = '''<!doctype html><meta charset="utf-8"><meta name="viewport" content="width=device-width">
<title>Toy Sim UI gallery</title><style>
body{background:#0f161f;color:#d9e2eb;font:16px system-ui;margin:24px}
h1,h2{color:#78b8ff}h2{font-size:18px;margin-top:32px}
.cards{display:grid;grid-template-columns:repeat(auto-fit,minmax(360px,1fr));gap:16px}
a{color:#74cfe3;background:#151f28;padding:8px;border:1px solid #364959;text-decoration:none}
img{width:100%;display:block}span{display:block;padding-top:8px;font-size:12px}
</style><h1>Toy Sim UI gallery</h1><p>32 reviewed workflows, rendered in software at 1600 × 900 and 900 × 650. Click an image for its full size. Workspace captures use the actual window chrome and populated fixtures on a plain background; the 3D scene is omitted.</p>'''
(gallery / 'index.html').write_text(document + ''.join(sections))
print(f'{len(sections)} workflows, {count} linked screenshots: {gallery / "index.html"}')
