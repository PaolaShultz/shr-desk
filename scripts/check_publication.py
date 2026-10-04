#!/usr/bin/env python3
"""Check Git blobs before publication; never print their contents or follow symlinks.

Default: inspect the complete index, including force-added and unchanged files.
--revision REF: inspect a committed tree. --push: inspect every outgoing commit
from Git's pre-push input, including files deleted again before the final commit.
The reviewed script/artwork list lives in scripts/publication-policy.json.
"""
import argparse
import hashlib
import json
from pathlib import PurePosixPath
import re
import subprocess
import sys


PRIVATE_PARTS = {
    'user', 'users', 'artifacts', 'artefacts', 'recordings', 'private',
    'local', 'scratch', 'tmp', 'target', '.venv', 'venv', '__pycache__',
    '.ruff_cache', '.pytest_cache', '.cache', '.shr-pa', 'node_modules',
}
MEDIA_SUFFIXES = {
    '.wav', '.wave', '.flac', '.mp3', '.aif', '.aiff', '.ogg', '.opus',
    '.m4a', '.aac', '.wma', '.raw', '.pcm', '.mp4', '.webm', '.mov', '.mkv',
    '.zip', '.tar', '.gz', '.tgz', '.xz', '.bz2', '.7z', '.rar', '.part',
    '.sqlite', '.sqlite3', '.db', '.pyc', '.pyo', '.pem', '.key', '.p12',
}
SCRIPT_SUFFIXES = {'.py', '.sh', '.bash', '.zsh', '.fish', '.ps1', '.js', '.ts', '.rb', '.pl'}
SECRET_PATTERNS = [
    re.compile(rb'-----BEGIN (?:[A-Z ]+ )?PRIVATE KEY-----'),
    re.compile(rb'\bgh[pousr]_[A-Za-z0-9]{36,}\b'),
    re.compile(rb'\bgithub_pat_[A-Za-z0-9_]{40,}\b'),
    re.compile(rb'\bAKIA[A-Z0-9]{16}\b'),
    re.compile(rb'\bsk-(?:proj-|svcacct-)?[A-Za-z0-9_-]{40,}\b'),
]
POLICY = 'scripts/publication-policy.json'


def git(*args):
    return subprocess.check_output(['git', *args])


def entries(revision=None):
    if revision is None:
        for record in git('ls-files', '--stage', '-z').split(b'\0'):
            if record:
                meta, path = record.split(b'\t', 1)
                mode, oid, stage = meta.decode().split()
                if stage != '0':
                    raise ValueError('Resolve unmerged index entries before publishing')
                yield path.decode(), mode, oid
    else:
        for record in git('ls-tree', '-r', '-z', revision).split(b'\0'):
            if record:
                meta, path = record.split(b'\t', 1)
                mode, _, oid = meta.decode().split()
                yield path.decode(), mode, oid


def reasons(path, mode, data, policy):
    p = PurePosixPath(path)
    parts = {part.lower() for part in p.parts}
    result = []
    if parts & PRIVATE_PARTS or p.name.lower().startswith('.env'):
        result.append('private/generated path')
    if p.suffix.lower() in MEDIA_SUFFIXES:
        result.append('media, archive, cache or credential file')
    if mode not in ('100644', '100755'):
        result.append('symlink or submodule requires an explicit publication contract')
        return result
    if len(p.parts) == 1 and path not in policy['root_files']:
        result.append('unreviewed root file')
    if len(p.parts) > 1 and p.parts[0] not in policy['directories']:
        result.append('unreviewed top-level directory')
    shebang = data.startswith(b'#!') and not (p.suffix == '.rs' and data.startswith(b'#!['))
    script = (mode == '100755' or p.suffix.lower() in SCRIPT_SUFFIXES or shebang
              or p.parts[0] == '.githooks')
    if script and path not in policy['scripts']:
        result.append('script absent from reviewed publication list')
    if data[:4] in (b'RIFF', b'RF64', b'fLaC', b'OggS', b'PK\x03\x04') or data[:3] == b'ID3':
        result.append('media/archive signature, regardless of filename')
    if len(data) > 5 * 1024 * 1024:
        result.append('file exceeds 5 MiB review limit')
    asset = policy.get('binary_assets', {}).get(path)
    if asset is not None:
        if (len(data) != asset['size'] or not data.startswith(bytes.fromhex(asset['magic_hex']))
                or hashlib.sha256(data).hexdigest() != asset['sha256']):
            result.append('reviewed binary asset identity mismatch')
    elif path in policy['binary_artwork']:
        if not data.startswith(b'\x89PNG\r\n\x1a\n'):
            result.append('reviewed PNG artwork has an unexpected format')
    else:
        try:
            data.decode('utf-8')
            if b'\0' in data:
                result.append('binary content outside reviewed artwork')
        except UnicodeDecodeError:
            result.append('binary content outside reviewed artwork')
    if any(pattern.search(data) for pattern in SECRET_PATTERNS):
        result.append('credential-like content; inspect locally')
    return result


def audit(revision=None, fallback_policy=None):
    files = list(entries(revision))
    policy_oid = next((oid for path, _, oid in files if path == POLICY), None)
    if policy_oid:
        policy = json.loads(git('cat-file', 'blob', policy_oid))
    elif fallback_policy is not None:
        policy = fallback_policy  # historical commit before this guard existed
    else:
        raise ValueError('Stage the reviewed publication policy before checking the index')
    failures = []
    for path, mode, oid in files:
        data = git('cat-file', 'blob', oid) if mode in ('100644', '100755', '120000') else b''
        for reason in reasons(path, mode, data, policy):
            failures.append(f'{revision or "index"}: {path}: {reason}')
    by_path = {path: (mode, oid) for path, mode, oid in files}
    for path, asset in policy.get('binary_assets', {}).items():
        if path not in by_path:
            continue
        for companion, expected in asset.get('required_files', {}).items():
            entry = by_path.get(companion)
            if (entry is None or entry[0] not in ('100644', '100755')
                    or hashlib.sha256(git('cat-file', 'blob', entry[1])).hexdigest() != expected):
                failures.append(f'{revision or "index"}: {path}: missing or changed required notice {companion}')
    return failures, len(files), policy


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group()
    group.add_argument('--revision')
    group.add_argument('--push', action='store_true')
    args = parser.parse_args()
    try:
        if args.push:
            index_failures, _, policy = audit()
            revisions = set()
            for line in sys.stdin:
                _, local, _, remote = line.split()
                if set(local) == {'0'}:
                    continue
                if not all(re.fullmatch(r'[0-9a-f]{40,64}', x) for x in (local, remote)):
                    raise ValueError('Invalid pre-push object ID')
                spec = local if set(remote) == {'0'} else f'{remote}..{local}'
                revisions.update(git('rev-list', spec).decode().splitlines())
            failures = list(index_failures)
            count = 0
            for revision in sorted(revisions):
                errors, n, _ = audit(revision, policy)
                failures.extend(errors)
                count += n
        else:
            failures, count, _ = audit(args.revision)
        if failures:
            print('Publication blocked:\n' + '\n'.join(failures), file=sys.stderr)
            return 1
        print(f'Publication check passed ({count} Git file entries).')
        return 0
    except (ValueError, KeyError, subprocess.CalledProcessError) as error:
        print(f'Publication check could not complete: {error}', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
