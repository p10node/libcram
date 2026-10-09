import hashlib
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest import mock
import package_sdk


class PackageTests(unittest.TestCase):
    def test_archive_contains_exact_bytes_and_verified_manifest(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / 'sdk'
            root.mkdir()
            (root / 'lib').mkdir()
            (root / 'lib/a').write_bytes(b'fixture-library-A')
            (root / 'header.h').write_bytes(b'fixture-header-B')
            output = Path(tmp) / 'sdk.tar.gz'
            package_sdk.make_archive(root, output, {'commit': 'fixture'}, 1234)
            self.assertTrue(output.is_file(), "archive must be produced")
            with tarfile.open(output) as archive:
                self.assertEqual(sorted(archive.getnames()), ['sdk/MANIFEST.json', 'sdk/header.h', 'sdk/lib/a'])
                manifest = json.load(archive.extractfile('sdk/MANIFEST.json'))
                self.assertEqual(manifest['source']['commit'], 'fixture')
                self.assertEqual(manifest['sha256'], {
                    'header.h': hashlib.sha256(b'fixture-header-B').hexdigest(),
                    'lib/a': hashlib.sha256(b'fixture-library-A').hexdigest(),
                })
                self.assertEqual(archive.extractfile('sdk/lib/a').read(), b'fixture-library-A')
            with self.assertRaises(FileExistsError):
                package_sdk.make_archive(root, output, {}, 1234)

    def test_failed_archive_is_not_published(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / 'sdk'; root.mkdir()
            (root / 'file').write_bytes(b'owned fixture')
            output = Path(tmp) / 'sdk.tar.gz'
            with mock.patch('tarfile.TarFile.addfile', side_effect=OSError('owned injected I/O failure')):
                with self.assertRaises(OSError):
                    package_sdk.make_archive(root, output, {}, 123)
            self.assertFalse(output.exists(), 'failed bytes must not look like a published SDK')


if __name__ == '__main__':
    unittest.main()
