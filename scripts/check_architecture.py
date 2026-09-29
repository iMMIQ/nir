#!/usr/bin/env python3
"""Check actual Cargo package IDs, normal/build edges, and transitive boundaries."""
import json,subprocess
m=json.loads(subprocess.check_output(['cargo','metadata','--format-version','1','--locked','--all-features']))
packages={p['id']:p for p in m['packages']};nodes={n['id']:n for n in m['resolve']['nodes']};workspace=set(m['workspace_members'])
allowed={
'nir-format':set(),'nir-core':{'nir-format'},'nir-content':{'nir-format'},'nir-assets':{'nir-format'},'nir-presentation':{'nir-format'},
'nir-player':{'nir-format','nir-core','nir-content','nir-assets','nir-presentation'},'nir-render-wgpu':{'nir-format','nir-presentation'},'nir-platform-web':{'nir-format'},'nir-compiler':{'nir-format','nir-core','nir-content'},
'novelc':{'nir-compiler','nir-content','nir-format'},'player-web':{'nir-format','nir-core','nir-content','nir-assets','nir-presentation','nir-player','nir-render-wgpu','nir-platform-web'},'xtask':set()}
def edges(id):return [d['pkg'] for d in nodes[id]['deps'] if any(k['kind'] in (None,'build') and k.get('target') is None for k in d['dep_kinds'])]
def closure(id,seen=None):
 seen=set() if seen is None else seen
 for dep in edges(id):
  if dep not in seen:seen.add(dep);closure(dep,seen)
 return seen
errors=[]
allowed['nir-engine']={'nir-format','nir-content','nir-player','nir-presentation','nir-render-wgpu'}
allowed['player-web'].add('nir-engine')
allowed['player-windows']={'player-desktop'}
allowed['player-desktop']={'nir-engine','nir-format','nir-content','nir-player','nir-presentation','nir-render-wgpu'}
allowed['player-linux']={'player-desktop'}
allowed['player-android']={'player-desktop'}
allowed['nir-apk']=set()
allowed['nir-compiler'].add('nir-apk')
for id in workspace:
 name=packages[id]['name']
 if name not in allowed:errors.append(f'unknown workspace package {name}');continue
 for dep in edges(id):
  if dep in workspace and packages[dep]['name'] not in allowed[name]:errors.append(f'{name} -> {packages[dep]["name"]}')
 names={packages[x]['name'] for x in closure(id)}
 if name in ['nir-format','nir-core','nir-content','nir-assets','nir-presentation']:
  bad=names & {'wgpu','web-sys','wasm-bindgen','nir-player','nir-compiler'}
  if bad:errors.append(f'{name} leaks {bad}')
 if name in {'player-web','player-windows','player-linux','player-android','player-desktop','nir-engine'} and names & {'nir-compiler', 'nir-apk', 'hb-subset', 'bindgen'}:errors.append('compiler linked into player')
wgpus={packages[x]['version'] for x in packages if packages[x]['name']=='wgpu'}
if len(wgpus)>1:errors.append(f'multiple wgpu versions: {wgpus}')
if errors:raise SystemExit('\n'.join(errors))
print(f'PASS actual dependency graph: {len(workspace)} packages, wgpu {wgpus}')
