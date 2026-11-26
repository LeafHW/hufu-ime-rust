"""Adversarial package fixtures: no Windows registration or installer execution."""
import json
import struct
import tempfile
import unittest
from unittest.mock import patch
from pathlib import Path

import windows_package as package


def fixture_pe(path, machine):
    blob = bytearray(128)
    blob[:2] = b'MZ'
    struct.pack_into('<I', blob, 60, 64)
    blob[64:68] = b'PE\0\0'
    struct.pack_into('<H', blob, 68, machine)
    path.write_bytes(blob)


def payload(root, arch):
    root.mkdir(parents=True)
    names = package.REQUIRED | {'version.txt'}
    if arch == 'arm64':
        names |= {'hufu-tsf-smoke-x64.exe', 'hufu_tsf_arm64_test.dll', 'hufu_tsf_x64_test.dll'}
    for name in names:
        machine = package.expected_machine(name, arch)
        if machine == 'data':
            (root / name).write_bytes(b'shared resource')
        else:
            fixture_pe(root / name, package.MACHINES[machine])
    package.make_manifest(root, arch, '1.9.9.10')


class PackageTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        # These fixtures test the manifest contract, not the hybrid PE format.
        self.hybrid = patch.object(package, 'verify_hybrid', return_value={})
        self.hybrid.start()

    def tearDown(self):
        self.hybrid.stop()
        self.temp.cleanup()

    def test_plain_arm64_is_not_a_hybrid(self):
        root = self.root / 'arm64'
        payload(root, 'arm64')
        self.hybrid.stop()
        with self.assertRaises(ValueError):
            package.validate_manifest(root)

    def test_wrong_os_component_rejected(self):
        root = self.root / 'arm64'
        payload(root, 'arm64')
        fixture_pe(root / 'hufu-server.exe', 0x8664)
        with self.assertRaisesRegex(ValueError, 'PE 架构'):
            package.make_manifest(root, 'arm64', '1.9.9.10')

    def test_changed_binary_rejected(self):
        root = self.root / 'arm64'
        payload(root, 'arm64')
        (root / 'hufu-server.exe').write_bytes(b'corrupt')
        with self.assertRaisesRegex(ValueError, 'SHA256'):
            package.validate_manifest(root)

    def test_missing_x64_view_rejected(self):
        root = self.root / 'arm64'
        payload(root, 'arm64')
        (root / 'hufu_tsf_x64_test.dll').unlink()
        with self.assertRaisesRegex(ValueError, '唯一实现'):
            package.make_manifest(root, 'arm64', '1.9.9.10')

    def test_traversal_and_inno_injection_rejected(self):
        for path in ('../data', 'x/../data', 'C:/data', '\\\\host\\share', '/rooted', '\\rooted', 'x:stream', 'x; y', '{app}/x', 'x\nfile', 'x./y', 'NUL.txt', 'a/COM1', 'a//b'):
            with self.subTest(path=path), self.assertRaises(ValueError):
                package.relative_path(path)

    def test_case_collision_rejected(self):
        root = self.root / 'x64'
        payload(root, 'x64')
        p = root / package.MANIFEST
        data = json.loads(p.read_text())
        data['files'].append(dict(data['files'][0], path=data['files'][0]['path'].upper()))
        p.write_text(json.dumps(data))
        with self.assertRaisesRegex(ValueError, '重复'):
            package.validate_manifest(root)

    def test_update_limit_includes_exact_boundary(self):
        f = self.root / 'HuFuSetup-1.9.9.10.exe'
        with f.open('wb') as stream:
            stream.truncate(package.UPDATE_LIMIT)
        self.assertEqual(package.check_update_artifact(f)['size'], package.UPDATE_LIMIT)
        with f.open('r+b') as stream:
            stream.truncate(package.UPDATE_LIMIT + 1)
        with self.assertRaises(ValueError):
            package.check_update_artifact(f)

    def test_dedup_and_upgrade_data_preservation(self):
        for arch in ('x64', 'arm64'):
            payload(self.root / 'payload' / arch, arch)
        data = self.root / 'data'
        for name in ('码表/方案/字典.txt', '数据/config.json', '数据/音效/key.wav'):
            p = data / name
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_bytes(b'shared resource')
        out = self.root / 'stage'
        result = package.stage(self.root / 'payload', data, out, '1.9.9.10')
        self.assertLess(result['unique_bytes'], result['expanded_bytes'])
        upgrade = (out / 'hufu-files-upgrade.lst').read_text(encoding='utf-8-sig')
        self.assertNotIn('码表', upgrade)
        self.assertTrue(all(x.startswith('!') for x in upgrade.splitlines()))
        self.assertIn('IsArm64System', (out / 'staging-programs.inc').read_text(encoding='utf-8-sig'))
        with self.assertRaisesRegex(ValueError, '必须为空'):
            package.stage(self.root / 'payload', data, out, '1.9.9.10')

    def test_user_adjustment_cannot_ship(self):
        for arch in ('x64', 'arm64'):
            payload(self.root / 'payload' / arch, arch)
        data = self.root / 'data'
        (data / '码表').mkdir(parents=True)
        (data / '数据').mkdir()
        (data / '码表/user-adjust.log').write_text('private')
        with self.assertRaisesRegex(ValueError, '用户数据'):
            package.stage(self.root / 'payload', data, self.root / 'stage', '1.9.9.10')


if __name__ == '__main__':
    unittest.main()
