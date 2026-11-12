#!/usr/bin/env python3
"""Package native release binaries; run on the same architecture as the build."""
import argparse
from pathlib import Path
import re
import struct
import subprocess
import tarfile

TARGETS = (
    'aarch64-apple-darwin', 'x86_64-apple-darwin',
    'aarch64-unknown-linux-musl', 'x86_64-unknown-linux-musl',
)


def verify_binary(path, target):
    data = path.read_bytes()
    arch = target.split('-')[0]
    if target.endswith('linux-musl'):
        if data[:6] != b'\x7fELF\x02\x01' or len(data) < 64:
            raise RuntimeError(f'{path}: expected a 64-bit Linux ELF binary')
        machine = struct.unpack_from('<H', data, 18)[0]
        if machine != {'x86_64': 62, 'aarch64': 183}[arch]:
            raise RuntimeError(f'{path}: ELF architecture differs from {target}')
        offset = struct.unpack_from('<Q', data, 32)[0]
        entry, count = struct.unpack_from('<HH', data, 54)
        if entry < 4 or offset + entry*count > len(data):
            raise RuntimeError(f'{path}: invalid ELF program headers')
        if any(struct.unpack_from('<I', data, offset+i*entry)[0] == 3 for i in range(count)):
            raise RuntimeError(f'{path}: Linux release binary must be static (no dynamic interpreter)')
    else:
        if data[:4] != b'\xcf\xfa\xed\xfe' or len(data) < 32:
            raise RuntimeError(f'{path}: expected a 64-bit macOS Mach-O binary')
        if struct.unpack_from('<I', data, 4)[0] != {'x86_64': 0x1000007, 'aarch64': 0x100000c}[arch]:
            raise RuntimeError(f'{path}: Mach-O architecture differs from {target}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--target', choices=TARGETS, required=True)
    parser.add_argument('--version', required=True)
    parser.add_argument('--bin-dir', type=Path, required=True)
    parser.add_argument('--output', type=Path, default=Path('dist'))
    args = parser.parse_args()
    if not re.fullmatch(r'v\d+\.\d+\.\d+(?:-[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?', args.version):
        parser.error('version must be a tag such as v0.1.0')
    root = Path(__file__).resolve().parents[1]
    files = {name: args.bin_dir/name for name in ('agenthub', 'ah')}
    for name, path in files.items():
        if path.is_symlink() or not path.is_file():
            raise RuntimeError(f'{path}: expected a regular executable file')
        verify_binary(path, args.target)
        actual = subprocess.check_output([str(path.resolve()), '--version'], text=True).strip()
        if actual != f'agenthub {args.version[1:]}':
            raise RuntimeError(f'{name} version differs from tag: {actual!r}')
    files.update({name: root/name for name in ('LICENSE', 'THIRD_PARTY_NOTICES.md')})
    args.output.mkdir(parents=True, exist_ok=True)
    archive = args.output/f'agenthub-{args.target}.tar.gz'
    with tarfile.open(archive, 'w:gz') as tar:
        for name, path in files.items():
            info = tar.gettarinfo(str(path), arcname=name)
            info.uid = info.gid = 0
            info.uname = info.gname = ''
            info.mode = 0o755 if name in ('ah', 'agenthub') else 0o644
            info.mtime = 0
            with path.open('rb') as stream:
                tar.addfile(info, stream)
    print(archive)


if __name__ == '__main__':
    main()
