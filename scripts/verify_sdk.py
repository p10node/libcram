#!/usr/bin/env python3
"""Verify an actual SDK archive and run native consumers in an owned prefix."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tarfile
import tempfile


def main():
    if not __debug__:
        raise RuntimeError('do not disable acceptance assertions with Python -O')
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('archive', type=Path)
    args = parser.parse_args()
    repo = Path(__file__).resolve().parent.parent
    with tempfile.TemporaryDirectory(prefix='libcram-sdk-verify-') as tmp:
        root = Path(tmp)
        with tarfile.open(args.archive) as archive:
            members = archive.getmembers()
            if any(not m.isfile() for m in members) or len({m.name for m in members}) != len(members):
                raise RuntimeError('package contains non-files or duplicate entries')
            archive.extractall(root, filter='data')
        prefixes = list(root.iterdir())
        assert len(prefixes) == 1 and prefixes[0].is_dir()
        prefix = prefixes[0]
        manifest = json.loads((prefix / 'MANIFEST.json').read_text())
        actual = {p.relative_to(prefix).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest()
                  for p in prefix.rglob('*') if p.is_file() and p != prefix / 'MANIFEST.json'}
        assert actual == manifest['sha256'], 'package bytes differ from manifest'
        env = os.environ.copy()
        # The extracted archive, never the repository build directory, owns the lookup.
        env['PKG_CONFIG_PATH'] = str(prefix / 'lib/pkgconfig')
        for key in ('DYLD_LIBRARY_PATH', 'DYLD_FALLBACK_LIBRARY_PATH', 'LD_LIBRARY_PATH',
                    'LD_PRELOAD', 'DYLD_INSERT_LIBRARIES', 'CPATH', 'C_INCLUDE_PATH',
                    'CPLUS_INCLUDE_PATH', 'LIBRARY_PATH'):
            env.pop(key, None)
        env['LIBCRAM_EXPECTED_LIBRARY'] = str(prefix / 'lib' / ('libcram.dylib' if sys.platform == 'darwin' else 'libcram.so'))
        flags = shlex.split(subprocess.check_output(
            ['pkg-config', '--cflags', '--libs', 'libcram'], env=env, text=True))
        for compiler, source, extra in [
            ('cc', 'package_smoke.c', ['-std=c11']),
            ('c++', 'package_smoke.cpp', ['-std=c++17'] + (['-mmacosx-version-min=14.0'] if sys.platform == 'darwin' else [])),
        ]:
            if sys.platform == 'linux':
                extra += ['-D_GNU_SOURCE', '-ldl']
            binary = root / f'{compiler.replace("+", "p")}-smoke'
            subprocess.run([compiler, *extra, '-Wall', '-Wextra', '-Werror',
                            str(repo / 'examples' / source), *flags,
                            f'-Wl,-rpath,{prefix / "lib"}', '-o', str(binary)], env=env, check=True)
            subprocess.run([str(binary)], env=env, check=True, timeout=20)
        print('SDK_MANIFEST_AND_NATIVE_CONSUMERS_PASS', manifest['source']['commit'])


if __name__ == '__main__':
    main()
