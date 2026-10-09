#!/usr/bin/env python3
"""Build a relocatable native C/C++ SDK; no publish or global installation.

The archive is an engineering build, not proof of hardware/performance acceptance.
"""
import argparse
import gzip
import hashlib
import io
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile


def make_archive(root, output, source, epoch):
    """Archive owned staging files, hashes and provenance without overwriting."""
    payload = {}
    for path in sorted(root.rglob('*')):
        if path.is_symlink():
            raise ValueError('symlink in SDK staging tree')
        if path.is_file():
            payload[path.relative_to(root).as_posix()] = path.read_bytes()
    if 'MANIFEST.json' in payload:
        raise ValueError('reserved manifest path')
    manifest = {'source': source, 'sha256': {
        name: hashlib.sha256(data).hexdigest() for name, data in payload.items()
    }}
    payload['MANIFEST.json'] = (json.dumps(manifest, indent=2, sort_keys=True) + '\n').encode()
    with output.open('xb') as raw:
        with gzip.GzipFile(filename='', fileobj=raw, mode='wb', mtime=epoch) as zipped:
            with tarfile.open(fileobj=zipped, mode='w') as archive:
                for name, data in sorted(payload.items()):
                    info = tarfile.TarInfo(f'{root.name}/{name}')
                    info.size, info.mtime, info.mode = len(data), epoch, 0o644
                    archive.addfile(info, io.BytesIO(data))


def command(root, *args):
    return subprocess.check_output(args, cwd=root, text=True).strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output-dir', type=Path, default=Path('target/dist'))
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    if command(root, 'git', 'status', '--porcelain'):
        raise RuntimeError('refusing to package a dirty source tree')
    commit = command(root, 'git', 'rev-parse', 'HEAD')
    epoch = int(command(root, 'git', 'show', '-s', '--format=%ct', 'HEAD'))
    metadata = json.loads(command(root, 'cargo', 'metadata', '--no-deps', '--format-version', '1', '--locked'))
    version = next(p['version'] for p in metadata['packages'] if p['name'] == 'libcram-ffi')
    rustc = command(root, 'rustc', '-vV')
    host = next(line.removeprefix('host: ') for line in rustc.splitlines() if line.startswith('host: '))
    if sys.platform not in ('darwin', 'linux'):
        raise RuntimeError('native archive supports Linux and macOS only')
    target = Path(metadata['target_directory']) / host / 'release'
    subprocess.run(['cargo', 'build', '-p', 'libcram-ffi', '--release', '--locked', '--target', host], cwd=root, check=True)
    if command(root, 'git', 'status', '--porcelain') or command(root, 'git', 'rev-parse', 'HEAD') != commit:
        raise RuntimeError('source changed during SDK build')
    out = args.output_dir.resolve()
    out.mkdir(parents=True, exist_ok=True)
    name = f'libcram-{version}-{host}-{commit[:12]}'
    archive_path = out / f'{name}.tar.gz'
    with tempfile.TemporaryDirectory(prefix='libcram-package-') as temporary:
        stage = Path(temporary) / name
        (stage / 'include').mkdir(parents=True)
        (stage / 'lib/pkgconfig').mkdir(parents=True)
        for filename in ('libcram.h', 'libcram.hpp'):
            shutil.copyfile(root / 'include' / filename, stage / 'include' / filename)
        shutil.copyfile(root / 'bindings/cpp/cram_resource.hpp', stage / 'include/cram_resource.hpp')
        extension = 'dylib' if sys.platform == 'darwin' else 'so'
        shutil.copyfile(target / f'libcram.{extension}', stage / 'lib' / f'libcram.{extension}')
        if sys.platform == 'darwin':
            library = stage / 'lib/libcram.dylib'
            # Cargo's absolute build-tree install name would bypass the SDK.
            subprocess.run(['install_name_tool', '-id', '@rpath/libcram.dylib', str(library)], check=True)
            subprocess.run(['codesign', '--force', '--sign', '-', str(library)], check=True)
        shutil.copyfile(target / 'libcram.a', stage / 'lib/libcram.a')
        for filename in ('README.md', 'LICENSE-MIT', 'LICENSE-APACHE'):
            shutil.copyfile(root / filename, stage / filename)
        shutil.copytree(root / 'docs', stage / 'docs')
        pc = ('prefix=${pcfiledir}/../..\nlibdir=${prefix}/lib\nincludedir=${prefix}/include\n\n'
              'Name: libcram\nDescription: Linux NUMA tier orchestration SDK\n'
              f'Version: {version}\nLibs: -L${{libdir}} -lcram\nCflags: -I${{includedir}}\n')
        (stage / 'lib/pkgconfig/libcram.pc').write_text(pc)
        make_archive(stage, archive_path, {'commit': commit, 'rustc': rustc, 'target': host,
                     'acceptance': 'engineering build; see bundled docs for open gates'}, epoch)
    print(archive_path)
    print('SHA256', hashlib.sha256(archive_path.read_bytes()).hexdigest())


if __name__ == '__main__':
    main()
