#!/usr/bin/env python3
"""Exercise the real installer against controlled archives, without network access."""
import hashlib
import io
import json
import re
import sys
import os
from pathlib import Path
import shutil
import subprocess
import struct

from package_release import verify_binary
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='agenthub-install-test-')
        self.root = Path(self.temp.name)
        self.bin = self.root/'tools'
        self.bin.mkdir()
        self.dest = self.root/'installed commands'
        self.assets = self.root/'assets'
        self.assets.mkdir()
        self.log = self.root/'requests'
        self.env = dict(os.environ, PATH=str(self.bin)+os.pathsep+os.environ['PATH'],
                        AH_TEST_OS='Linux', AH_TEST_ARCH='x86_64', AH_TEST_ASSETS=str(self.assets),
                        AH_TEST_LOG=str(self.log), AGENTHUB_INSTALL_DIR=str(self.dest),
                        AH_TEST_PYTHON=shutil.which('python3'))
        self.stub('uname', '#!/bin/sh\ncase "$1" in -s) echo "$AH_TEST_OS";; -m) echo "$AH_TEST_ARCH";; esac\n')
        self.stub('curl', '''#!/bin/sh
url=; out=
while [ "$#" -gt 0 ]; do
 case "$1" in
  --output) out=$2; shift 2;;
  https://*) url=$1; shift;;
  *) shift;;
 esac
done
printf '%s\\n' "$url" >> "$AH_TEST_LOG"
cp "$AH_TEST_ASSETS/${url##*/}" "$out"
''')

    def tearDown(self):
        self.temp.cleanup()

    def stub(self, name, text):
        path = self.bin/name
        path.write_text(text)
        path.chmod(0o755)

    def archive(self, target, missing=False, corrupt=False):
        name = f'agenthub-{target}.tar.gz'
        path = self.assets/name
        with tarfile.open(path, 'w:gz') as archive:
            for member in ['agenthub'] if missing else ['agenthub', 'ah']:
                data = b'#!/bin/sh\nprintf "agenthub test binary\\n"\n'
                info = tarfile.TarInfo(member)
                info.size = len(data)
                info.mode = 0o755
                archive.addfile(info, io.BytesIO(data))
        digest = '0'*64 if corrupt else hashlib.sha256(path.read_bytes()).hexdigest()
        (self.assets/'SHA256SUMS').write_text(f'{digest}  {name}\n')
        return name

    def run_install(self, *args, **env):
        return subprocess.run(['/bin/sh', str(ROOT/'install.sh'), *args], env=dict(self.env, **env),
                              capture_output=True, text=True, timeout=20)

    def test_four_platforms_and_architecture_aliases(self):
        for system, arch, target in [
            ('Darwin', 'arm64', 'aarch64-apple-darwin'),
            ('Darwin', 'x86_64', 'x86_64-apple-darwin'),
            ('Linux', 'aarch64', 'aarch64-unknown-linux-musl'),
            ('Linux', 'x86_64', 'x86_64-unknown-linux-musl'),
            ('Linux', 'amd64', 'x86_64-unknown-linux-musl'),
        ]:
            with self.subTest(system=system, arch=arch):
                name = self.archive(target)
                result = self.run_install(AH_TEST_OS=system, AH_TEST_ARCH=arch)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn('/releases/latest/download/'+name, self.log.read_text())
                for binary in ['agenthub', 'ah']:
                    path = self.dest/binary
                    self.assertTrue(os.access(path, os.X_OK))
                    self.assertEqual(subprocess.check_output([str(path)], text=True).strip(), 'agenthub test binary')
                self.assertFalse(list(self.dest.glob('.agenthub-install.*')))

    def test_offline_archives_never_use_download_or_build_tools(self):
        for system, arch, target in [('Linux', 'x86_64', 'x86_64-unknown-linux-musl'),
                                      ('Linux', 'aarch64', 'aarch64-unknown-linux-musl'),
                                      ('Darwin', 'arm64', 'aarch64-apple-darwin'),
                                      ('Darwin', 'x86_64', 'x86_64-apple-darwin')]:
            name = self.archive(target)
            # Prefer a per-archive checksum, as supplied with offline downloads.
            (self.assets/(name+'.sha256')).write_text((self.assets/'SHA256SUMS').read_text())
            (self.assets/'SHA256SUMS').unlink()
            result = self.run_install('--archive', str(self.assets/name), AH_TEST_OS=system, AH_TEST_ARCH=arch)
            self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.log.exists(), 'offline installation must not call curl')

    def test_offline_refuses_wrong_target_missing_checksum_or_ambiguous_options(self):
        name = self.archive('aarch64-apple-darwin')
        result = self.run_install('--archive', str(self.assets/name))
        self.assertNotEqual(result.returncode, 0)
        name = self.archive('x86_64-unknown-linux-musl')
        (self.assets/'SHA256SUMS').unlink()
        self.assertNotEqual(self.run_install('--archive', str(self.assets/name)).returncode, 0)
        for option in [('--from-source',), ('--version', 'v0.1.0')]:
            self.assertNotEqual(self.run_install('--archive', str(self.assets/name), *option).returncode, 0)
        self.assertFalse(self.log.exists())
        self.assertFalse(self.dest.exists())

    def test_shasum_path_without_gnu_sha256sum(self):
        # macOS normally provides shasum, not GNU sha256sum.
        for tool in ['tar', 'gzip', 'awk', 'mktemp', 'rm', 'mkdir', 'cp', 'mv', 'chmod', 'shasum', 'perl']:
            source = shutil.which(tool)
            self.assertIsNotNone(source, tool)
            (self.bin/tool).symlink_to(source)
        self.archive('aarch64-apple-darwin')
        result = self.run_install(PATH=str(self.bin), AH_TEST_OS='Darwin', AH_TEST_ARCH='arm64')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((self.dest/'ah').is_file())

    def test_pinned_version_and_explicit_destination(self):
        self.archive('x86_64-unknown-linux-musl')
        dest = self.root/'custom'
        result = self.run_install('--version', '0.1.0', '--bin-dir', str(dest))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('/releases/download/v0.1.0/', self.log.read_text())
        self.assertTrue((dest/'ah').is_file())
        self.assertFalse(self.dest.exists())

    def test_invalid_input_does_not_download(self):
        for args in [('--version', '../../main'), ('--version',), ('--unknown',)]:
            self.assertNotEqual(self.run_install(*args).returncode, 0)
        for system, arch in [('Windows', 'x86_64'), ('Linux', 'armv7l')]:
            self.assertNotEqual(self.run_install(AH_TEST_OS=system, AH_TEST_ARCH=arch).returncode, 0)
        self.assertFalse(self.log.exists())
        self.assertFalse(self.dest.exists())

    def test_bad_checksum_and_incomplete_archive_preserve_old_install(self):
        self.dest.mkdir()
        for name in ['ah', 'agenthub']:
            (self.dest/name).write_text('old installation')
        for kwargs in [dict(corrupt=True), dict(missing=True)]:
            self.archive('x86_64-unknown-linux-musl', **kwargs)
            result = self.run_install()
            self.assertNotEqual(result.returncode, 0)
            for name in ['ah', 'agenthub']:
                self.assertEqual((self.dest/name).read_text(), 'old installation')

    def test_failed_download_does_not_create_installation(self):
        result = self.run_install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('--from-source', result.stderr)
        self.assertFalse(self.dest.exists())

    def test_directory_collision_does_not_replace_other_command(self):
        self.archive('x86_64-unknown-linux-musl')
        (self.dest/'ah').mkdir(parents=True)
        (self.dest/'agenthub').write_text('old')
        self.assertNotEqual(self.run_install().returncode, 0)
        self.assertEqual((self.dest/'agenthub').read_text(), 'old')
        self.assertTrue((self.dest/'ah').is_dir())

    def test_source_build_uses_locked_dependencies_and_matching_ref(self):
        self.stub('git', '''#!/bin/sh
printf '%s\\n' "$*" >> "$AH_TEST_LOG"
for arg; do dest=$arg; done
mkdir -p "$dest/crates/cli"
''')
        self.stub('cargo', '''#!/bin/sh
printf '%s\\n' "$*" >> "$AH_TEST_LOG"
case " $* " in *' --locked '*) ;; *) exit 1;; esac
while [ "$#" -gt 0 ]; do
 if [ "$1" = --root ]; then dest=$2; break; fi
 shift
done
mkdir -p "$dest/bin"
printf '#!/bin/sh\\nexit 0\\n' > "$dest/bin/agenthub"
cp "$dest/bin/agenthub" "$dest/bin/ah"
''')
        result = self.run_install('--from-source', '--version', 'v0.1.0')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('--branch v0.1.0', self.log.read_text())
        self.assertIn('install --locked --path', self.log.read_text())
        self.assertTrue((self.dest/'ah').is_file())
        self.assertNotIn('/releases/', self.log.read_text())

    def test_failed_source_build_preserves_existing_installation(self):
        self.dest.mkdir()
        for name in ['agenthub', 'ah']:
            (self.dest/name).write_text('old')
        self.stub('git', '#!/bin/sh\nfor arg; do dest=$arg; done\nmkdir -p "$dest/crates/cli"\n')
        self.stub('cargo', '#!/bin/sh\nexit 1\n')
        self.assertNotEqual(self.run_install('--from-source').returncode, 0)
        self.assertEqual((self.dest/'ah').read_text(), 'old')
        self.assertEqual((self.dest/'agenthub').read_text(), 'old')

    def test_help_needs_no_home_or_network(self):
        self.assertEqual(self.run_install('--help', HOME='').returncode, 0)
        self.assertFalse(self.log.exists())


class PackageTests(unittest.TestCase):
    def test_elf_target_and_static_link_requirement(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)/'binary'
            for arch, machine in [('x86_64', 62), ('aarch64', 183)]:
                data = bytearray(120)
                data[:6] = b'\x7fELF\x02\x01'
                struct.pack_into('<H', data, 18, machine)
                struct.pack_into('<Q', data, 32, 64)
                struct.pack_into('<HH', data, 54, 56, 1)
                path.write_bytes(data)
                verify_binary(path, arch+'-unknown-linux-musl')
                other = 'aarch64' if arch == 'x86_64' else 'x86_64'
                with self.assertRaises(RuntimeError):
                    verify_binary(path, other+'-unknown-linux-musl')
                struct.pack_into('<I', data, 64, 3)
                path.write_bytes(data)
                with self.assertRaisesRegex(RuntimeError, 'must be static'):
                    verify_binary(path, arch+'-unknown-linux-musl')

    def test_cross_package_records_pending_execution_and_includes_offline_installer(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            data = bytearray(120)
            data[:6] = b'\x7fELF\x02\x01'
            struct.pack_into('<H', data, 18, 62)
            struct.pack_into('<Q', data, 32, 64)
            struct.pack_into('<HH', data, 54, 56, 1)
            for name in ['ah', 'agenthub']:
                (root/name).write_bytes(data)
            version = re.search(r'^version = "([^"]+)"', (ROOT/'Cargo.toml').read_text(), re.M)[1]
            result = subprocess.run([sys.executable, str(ROOT/'scripts/package_release.py'),
                '--target', 'x86_64-unknown-linux-musl', '--version', 'v'+version,
                '--bin-dir', str(root), '--output', str(root/'dist'), '--cross-build'],
                capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr)
            archive = root/'dist/agenthub-x86_64-unknown-linux-musl.tar.gz'
            with tarfile.open(archive) as package:
                self.assertEqual(set(package.getnames()), {'ah', 'agenthub', 'install.sh', 'BUILD-INFO.json', 'LICENSE', 'THIRD_PARTY_NOTICES.md'})
                self.assertFalse(json.load(package.extractfile('BUILD-INFO.json'))['native_version_check'])
            expected = hashlib.sha256(archive.read_bytes()).hexdigest()
            self.assertEqual(Path(str(archive)+'.sha256').read_text(), f'{expected}  {archive.name}\n')

    def test_macos_target_and_bad_archive_members(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)/'binary'
            for arch, machine in [('x86_64', 0x1000007), ('aarch64', 0x100000c)]:
                path.write_bytes(b'\xcf\xfa\xed\xfe'+struct.pack('<I', machine)+bytes(24))
                verify_binary(path, arch+'-apple-darwin')
                other = 'aarch64' if arch == 'x86_64' else 'x86_64'
                with self.assertRaises(RuntimeError):
                    verify_binary(path, other+'-apple-darwin')
                with self.assertRaises(RuntimeError):
                    verify_binary(path, arch+'-unknown-linux-musl')
            path.write_text('not a binary')
            with self.assertRaises(RuntimeError):
                verify_binary(path, 'aarch64-apple-darwin')


if __name__ == '__main__':
    unittest.main(verbosity=2)
