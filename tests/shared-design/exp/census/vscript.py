#!/usr/bin/env python3
"""List the constructs a GNU ld version script uses: nodes, inheritance,
global:/local: blocks, implicit-global entries, wildcards, extern blocks.
Run as: python3 -I vscript.py FILE"""
import re, sys
text = open(sys.argv[1]).read()
text = re.sub(r'/\*.*?\*/', ' ', text, flags=re.S)
text = re.sub(r'#[^\n]*', ' ', text)
nodes = re.findall(r'([A-Za-z0-9_.]*)\s*\{(.*?)\}\s*([A-Za-z0-9_.]*)\s*;', text, flags=re.S)
tot = {'global': 0, 'local': 0, 'implicit-global': 0, 'wildcards': [], 'extern': 0}
for name, body, parent in nodes:
    mode, counts = 'implicit-global', {'global': 0, 'local': 0, 'implicit-global': 0}
    if re.search(r'extern\s+"', body):
        tot['extern'] += 1
    for tok in re.findall(r'[^\s;]+:|[^\s;:]+(?=\s*;)', body):
        if tok in ('global:', 'local:'):
            mode = tok[:-1]; continue
        counts[mode] += 1
        if re.search(r'[*?\[]', tok):
            tot['wildcards'].append(f'{tok} ({mode} in {name or "<anonymous>"})')
    for k in counts:
        tot[k] += counts[k]
    print(f'node {name or "<anonymous>"}: parent={parent or "-"} global:{counts["global"]} local:{counts["local"]} entries-without-keyword:{counts["implicit-global"]}')
print(f'{len(nodes)} nodes; {sum(1 for n in nodes if n[2])} with a parent; totals: global {tot["global"]}, '
      f'local {tot["local"]}, without keyword (global) {tot["implicit-global"]}; wildcards: {tot["wildcards"] or "none"}; '
      f'extern "C++" blocks: {tot["extern"]}')
