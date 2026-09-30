#!/usr/bin/env python3
"""Check that every advertised capability has a release ledger row with
execution, restore and backend acceptance evidence (P6.3). A capability only
stays advertised while the ledger names where it executes, how its state
restores, and which real backend accepted it; Windows native stays 待验证
until a real machine passes it."""
import re
from pathlib import Path

root = Path(__file__).resolve().parent.parent
source = (root / 'crates/nir-format/src/lib.rs').read_text()
block = re.search(r'pub const CAPABILITIES[^=]*= &\[(.*?)\];', source, re.S)
if not block:
    raise SystemExit('CAPABILITIES array not found in crates/nir-format/src/lib.rs')
code = set(re.findall(r'"([^"]+\.v\d+)"', block.group(1)))

doc = (root / 'docs/CAPABILITIES.md').read_text()
section = re.search(r'\n## 能力发行清单\n(.*?)(?=\n## |\Z)', doc, re.S)
errors = []
rows = {}
if not section:
    errors.append('docs/CAPABILITIES.md is missing the 能力发行清单 section')
else:
    for line in section.group(1).splitlines():
        if not line.startswith('|'):
            continue
        cells = [c.strip() for c in line.strip().strip('|').split('|')]
        if len(cells) != 6 or set(cells[0]) <= {'-', ' '} or cells[0] == '能力':
            continue
        rows[cells[0]] = cells[1:]

COLUMNS = ('执行证据', '恢复证据', 'Web 后端', 'Windows 原生', '备注')
for capability in sorted(code & set(rows)):
    evidence = rows[capability]
    for name, cell in zip(COLUMNS, evidence):
        if not cell:
            errors.append(f'{capability}: empty {name} cell')
    if evidence[3] not in ('待验证', '✓'):
        errors.append(f'{capability}: Windows column must be 待验证 or ✓, not {evidence[3]!r}')
    for cell in evidence[:3]:
        for path in re.findall(r'(?:crates|tests|apps|scripts|docs)/[\w./-]+\.(?:rs|js)', cell):
            if not (root / path).exists():
                errors.append(f'{capability}: cited path does not exist: {path}')
for capability in sorted(code - set(rows)):
    errors.append(f'advertised in code but no ledger row: {capability}')
for capability in sorted(set(rows) - code):
    errors.append(f'ledger row for unknown capability: {capability}')
if errors:
    raise SystemExit('\n'.join(errors))
windows = sum(1 for c in rows.values() if c[3] == '✓')
print(f'PASS capability release ledger: {len(rows)} capabilities, '
      f'{len(rows) - windows} Windows-native pending real-machine acceptance')
