#!/usr/bin/env python3
"""Extracts the market data PodStack bundles from CCP's static data export (the JSONL edition).

Writes data/market_types.json (every published type on the market), data/market_groups.json (the market group
tree) and data/universe.json (known-space regions and solar systems, for order locations and security filters).

Download https://developers.eveonline.com/static-data/eve-online-static-data-latest-jsonl.zip, unzip it somewhere,
then run from the repository root: python3 -I tools/convert_sde.py <unzipped dir>
"""
import json
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / 'data'
SRC = pathlib.Path(sys.argv[1])


def rows(name):
    with open(SRC / f'{name}.jsonl', encoding='utf-8') as f:
        for line in f:
            yield json.loads(line)


def en(v):
    return v.get('en', '') if isinstance(v, dict) else (v or '')


def dump(name, value):
    (OUT / name).write_text(json.dumps(value, separators=(',', ':'), ensure_ascii=False))


groups = [[g['_key'], g.get('parentGroupID'), en(g['name'])] for g in rows('marketGroups')]
groups.sort()
dump('market_groups.json', groups)

types = []
for t in rows('types'):
    if not t.get('published') or not t.get('marketGroupID'):
        continue
    # Packaged volume is what a hauler moves; ships and containers are only traded packaged.
    volume = t.get('packagedVolume') or t.get('volume') or 0
    types.append([t['_key'], en(t['name']), t['marketGroupID'], round(volume, 4), t.get('metaGroupID')])
types.sort()
dump('market_types.json', types)

# Wormhole space (11xxxxxx) and the abyss have no markets.
regions = sorted([r['_key'], en(r['name'])] for r in rows('mapRegions') if r['_key'] < 11000000)
known = {r[0] for r in regions}
systems = sorted(
    [s['_key'], en(s['name']), s['regionID'], round(s.get('securityStatus', 0), 4)]
    for s in rows('mapSolarSystems') if s['regionID'] in known
)
dump('universe.json', {'regions': regions, 'systems': systems})
print(f'{len(groups)} market groups, {len(types)} types, {len(regions)} regions, {len(systems)} systems')
