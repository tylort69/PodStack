#!/usr/bin/env python3
"""Converts the legacy app's static data modules (legacy/resources/*.js) into JSON under data/.

The legacy modules are `export default <JSON>` files generated from EVE's static data export, so stripping the
export prefix leaves plain JSON. Run from the repository root: python3 tools/convert_legacy_data.py
"""
import json
import pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent
SRC = ROOT / 'legacy' / 'resources'
OUT = ROOT / 'data'


def load(name):
    text = (SRC / name).read_text()
    body = text[text.index('export default') + len('export default'):].strip().rstrip(';')
    return json.loads(body)


def dump(name, value):
    OUT.mkdir(exist_ok=True)
    (OUT / name).write_text(json.dumps(value, separators=(',', ':'), ensure_ascii=False))


skills = load('all_skills.js')
dump('skills.json', skills)
dump('alpha_caps.json', load('alpha_skill_set.js'))
dump('skill_prices.json', load('skill_base_prices.js'))

ships = load('ships.js')
# The 3D viewer's model, paint and nebula data aren't used by the native app.
for ship in ships['ships'].values():
    ship.pop('model', None)
dump('ships.json', {'ships': ships['ships'], 'races': ships['races']})
print('wrote', sorted(p.name for p in OUT.iterdir()))
