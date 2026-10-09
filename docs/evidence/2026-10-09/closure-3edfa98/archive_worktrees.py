from pathlib import Path
import datetime
import hashlib
import json
import os
import shutil
import subprocess

root = Path.cwd()
def git(*args, cwd=root):
    return subprocess.check_output(['git', *args], cwd=cwd, text=True).strip()
assert git('rev-parse', 'HEAD') == '3edfa98d95e039cdaca20e9906de3d10db301f2a'
assert not git('status', '--porcelain')
names = ['bridge', 'device', 'entropy', 'pebs', 'runtime']
paths = [root.parent / ('libcram-' + n) for n in names]
cwds = subprocess.run(['lsof', '-n', '-a', '-d', 'cwd', '-F', 'pn'], capture_output=True, text=True)
assert cwds.returncode in (0, 1), cwds.stderr
for path in paths:
    assert not any(line.startswith('n' + str(path)) for line in cwds.stdout.splitlines()), 'active cwd: ' + str(path)
archive = root.parent / 'libcram-worktree-archive' / datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ')
archive.mkdir(parents=True, exist_ok=False)

def digest(path):
    with path.open('rb') as file:
        return hashlib.file_digest(file, 'sha256').hexdigest()

def inventory(path):
    result = {}
    for entry in sorted(path.rglob('*')):
        rel = entry.relative_to(path).as_posix()
        if entry.is_symlink():
            result[rel] = {'link': os.readlink(entry)}
        elif entry.is_file():
            result[rel] = {'bytes': entry.stat().st_size, 'sha256': digest(entry)}
        elif entry.is_dir():
            result[rel] = {'directory': True}
        else:
            raise RuntimeError('unexpected special file: ' + str(entry))
    return result

receipts = []
for path in paths:
    assert not git('status', '--porcelain', '--untracked-files=all', cwd=path), str(path)
    cherry = git('cherry', 'main', 'HEAD', cwd=path)
    assert cherry and all(line.startswith('- ') for line in cherry.splitlines()), cherry
    dest = archive / path.name
    dest.mkdir()
    receipt = {'worktree': str(path), 'branch': git('branch', '--show-current', cwd=path), 'head': git('rev-parse', 'HEAD', cwd=path), 'patch_equivalence': cherry}
    subprocess.run(['git', 'archive', '--format=tar', '-o', str(dest / 'source.tar'), 'HEAD'], cwd=path, check=True)
    receipt['source_tar_sha256'] = digest(dest / 'source.tar')
    shutil.copytree(Path(git('rev-parse', '--absolute-git-dir', cwd=path)), dest / 'git-metadata')
    receipt['ignored'] = {}
    ignored = git('ls-files', '--others', '--ignored', '--exclude-standard', '--directory', cwd=path)
    for rel in ignored.splitlines():
        source = path / rel
        assert source.is_dir() and not source.is_symlink(), rel
        before = inventory(source)
        destination = dest / 'ignored' / rel
        destination.parent.mkdir(parents=True, exist_ok=True)
        source.rename(destination)
        assert inventory(destination) == before, 'archive mismatch: ' + rel
        receipt['ignored'][rel] = before
    assert not git('status', '--porcelain', '--ignored', cwd=path)
    (dest / 'manifest.json').write_text(json.dumps(receipt, indent=2) + '\n')
    subprocess.run(['git', 'worktree', 'remove', str(path)], cwd=root, check=True)
    receipts.append({'name': path.name, 'manifest': str(dest / 'manifest.json'), 'branch_retained': receipt['branch']})
    print('ARCHIVED_AND_REMOVED', path, flush=True)
(archive / 'index.json').write_text(json.dumps(receipts, indent=2) + '\n')
print('ARCHIVE', archive)
print(git('worktree', 'list', '--porcelain'))
