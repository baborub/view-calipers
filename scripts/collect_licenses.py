"""Collect redistributable notices for the locked Windows dependency graph."""
from pathlib import Path
import argparse
import json
import shutil
import subprocess
import urllib.request
from concurrent.futures import ThreadPoolExecutor

parser = argparse.ArgumentParser()
parser.add_argument('--metadata', type=Path)
args = parser.parse_args()
root = Path(__file__).resolve().parents[1]
metadata = json.loads(args.metadata.read_text(encoding='utf-8-sig')) if args.metadata else json.loads(
    subprocess.check_output(['cargo', 'metadata', '--locked', '--filter-platform', 'x86_64-pc-windows-msvc', '--format-version', '1'], cwd=root))
packages = {p['id']: p for p in metadata['packages']}
nodes = {n['id']: n for n in metadata['resolve']['nodes']}
seen = set()
def visit(identifier):
    if identifier in seen:
        return
    seen.add(identifier)
    for dep in nodes[identifier]['deps']:
        visit(dep['pkg'])
visit(metadata['resolve']['root'])
out = root / 'licenses'
out.mkdir(exist_ok=True)
rows = ['# Third-party notices', '',
        'This inventory includes the resolved Windows dependency graph and build-time crates from Cargo.lock. Each dependency retains its own license; MIT covers only the view-calipers application source.', '',
        'Full notices are preserved under `licenses/`. UnRAR additionally requires the conditions in [UnRAR-LICENSE.txt](UnRAR-LICENSE.txt). FFmpeg and Blender are external and are not shipped in the release archive.', '',
        '| Package | Version | Declared license | License files |', '| --- | --- | --- | --- |']
missing = []
upstream = {}
for package in sorted((packages[i] for i in seen if packages[i]['source']), key=lambda p: (p['name'], p['version'])):
    base = Path(package['manifest_path']).parent
    destination = out / (package['name'] + '-' + package['version'])
    files = set()
    for pattern in ['LICENSE*', 'LICENCE*', 'COPYING*', 'COPYRIGHT*', 'NOTICE*', 'UNLICENSE*', 'license*', 'licence*', 'licenses/**/*']:
        files.update(p for p in base.glob(pattern) if p.is_file())
    if package.get('license_file'):
        path = base / package['license_file']
        if path.is_file():
            files.add(path)
    for path in base.rglob('*'):
        if path.is_file() and any(token in path.name.upper() for token in ['LICENSE','LICENCE','COPYRIGHT','COPYING','NOTICE','OFL.TXT','UFL.TXT']):
            files.add(path)
    links = []
    for path in sorted(files):
        relative = path.relative_to(base)
        target = destination / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(path, target)
        links.append(f'[{relative.as_posix()}]({target.relative_to(root).as_posix()})')
    if not links or package['name'] == 'epaint_default_fonts':
        missing.append(package['name'])
        repository = (package.get('repository') or '').rstrip('/')
        if repository.startswith('https://github.com/'):
            repository = '/'.join(repository.split('/')[:5])
            vcs = base / '.cargo_vcs_info.json'
            commit = json.loads(vcs.read_text())['git']['sha1'] if vcs.exists() else 'HEAD'
            upstream.setdefault((repository,commit), []).append((package,destination))
    rows.append(f"| {package['name']} | {package['version']} | {package.get('license') or 'See notices'} | {', '.join(links) or 'No license file in crate archive'} |")
def fetch_notices(item):
    (repository, commit), entries = item
    raw = repository.replace('https://github.com/','https://raw.githubusercontent.com/') + '/' + commit + '/'
    found = []
    for name in ['LICENSE-MIT','LICENSE-APACHE','LICENSE','LICENSE.txt','LICENSE.md','COPYING','NOTICE','LICENSE-MIT.txt','LICENSE-APACHE.txt']:
        try:
            with urllib.request.urlopen(raw + name, timeout=8) as response:
                data=response.read()
            if not data or b'<html' in data[:200].lower():
                continue
            found.append((name,data,raw+name))
            if name in ['LICENSE','LICENSE.txt','LICENSE.md','COPYING']:
                break
        except Exception:
            pass
    return entries,found

with ThreadPoolExecutor(max_workers=12) as pool:
    for entries, notices in pool.map(fetch_notices, upstream.items()):
        for package, destination in entries:
            if notices:
                destination.mkdir(parents=True,exist_ok=True)
                sources=[]
                for name,data,url in notices:
                    (destination / ('UPSTREAM-' + name)).write_bytes(data)
                    sources.append(url)
                (destination / 'UPSTREAM-SOURCES.txt').write_text('\n'.join(sources)+'\n',encoding='utf-8')
                missing.remove(package['name'])
                for index,row in enumerate(rows):
                    if row.startswith('| '+package['name']+' |'):
                        links=', '.join(f'[{p.name}]({p.relative_to(root).as_posix()})' for p in sorted(destination.rglob('*')) if p.is_file())
                        rows[index]=f"| {package['name']} | {package['version']} | {package.get('license') or 'See notices'} | {links} |"

# Some crate archives declare a standard license but omit its full text.
# Preserve that declared license and its metadata attribution with the SPDX text.
standard_texts = {}
for name in list(missing):
    package = next(p for p in packages.values() if p['name'] == name)
    declared = package.get('license') or ''
    selected = next((candidate for candidate in ['Apache-2.0','CC0-1.0','MIT'] if candidate in declared), None)
    if not selected:
        continue
    url = f'https://raw.githubusercontent.com/spdx/license-list-data/main/text/{selected}.txt'
    if selected not in standard_texts:
        with urllib.request.urlopen(url,timeout=20) as response:
            standard_texts[selected]=response.read().decode('utf-8')
    destination=out/(package['name']+'-'+package['version'])
    destination.mkdir(exist_ok=True)
    authors=', '.join(package.get('authors') or [])
    notice=f"Package: {name} {package['version']}\nDeclared license: {declared}\nAuthors from Cargo metadata: {authors or 'Not listed'}\nSource: {package.get('repository') or 'crates.io package source'}\nStandard license text: {url}\n"
    (destination/'PACKAGE-NOTICE.txt').write_text(notice,encoding='utf-8')
    text=standard_texts[selected]
    if selected=='MIT' and package.get('authors'):
        holders=', '.join(author.split('<')[0].strip() for author in package['authors'])
        text=text.replace('Copyright (c) <year> <copyright holders>', 'Copyright (c) '+holders)
    (destination/(selected+'.txt')).write_text(text,encoding='utf-8')
    missing.remove(name)
    for index,row in enumerate(rows):
        if row.startswith('| '+name+' |'):
            links=', '.join(f'[{p.name}]({p.relative_to(root).as_posix()})' for p in sorted(destination.rglob('*')) if p.is_file())
            rows[index]=f"| {name} | {package['version']} | {declared} | {links} |"

(root / 'THIRD_PARTY.md').write_text('\n'.join(rows) + '\n', encoding='utf-8')
print(f'Collected notices for {len(seen) - 1} dependencies')
if missing:
    print('Review crates without root notice files: ' + ', '.join(missing))
    raise SystemExit(1)
