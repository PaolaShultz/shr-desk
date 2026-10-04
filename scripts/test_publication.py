"""Publication checks against real temporary Git indexes and commit histories."""
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

import check_publication


class PublicationTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.script = Path(check_publication.__file__).resolve()
        self.git('init', '-q')
        self.git('config', 'user.name', 'Synthetic Test')
        self.git('config', 'user.email', 'test@example.invalid')
        self.write('scripts/publication-policy.json', json.dumps({
            'root_files': ['README.md'], 'directories': ['src', 'scripts', 'docs'],
            'scripts': [], 'binary_artwork': [],
        }).encode())
        self.write('src/lib.rs', b'// public code\n')
        self.git('add', '.')

    def git(self, *args):
        return subprocess.check_output(['git', *args], cwd=self.root, stderr=subprocess.PIPE)

    def write(self, path, data):
        file = self.root / path
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_bytes(data)

    def check(self, *args, stdin=None):
        return subprocess.run(['python3', str(self.script), *args], cwd=self.root,
                              input=stdin, capture_output=True, text=True)

    def test_private_paths_and_unreviewed_scripts_are_blocked_even_when_force_added(self):
        self.assertEqual(self.check().returncode, 0)
        for name in ('user/settings.json', 'docs/Artefacts/results.json', 'scripts/scratch.py', 'scripts/unreviewed'):
            with self.subTest(name=name):
                self.write(name, b'{}')
                if name == 'scripts/unreviewed':
                    (self.root / name).chmod(0o755)
                self.git('add', '-f', name)
                result = self.check()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(name, result.stderr)
                self.git('rm', '--cached', name)

    def test_staged_secret_is_checked_even_when_worktree_is_clean(self):
        secret = b'gh' + b'p_' + b'A' * 40
        self.write('src/lib.rs', secret)
        self.git('add', 'src/lib.rs')
        self.write('src/lib.rs', b'// safe worktree\n')
        result = self.check()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('credential-like', result.stderr)
        self.assertNotIn(secret.decode(), result.stderr)

    def test_rust_inner_attribute_is_not_a_shebang_but_scripts_still_are(self):
        self.write('src/lib.rs', b'#![cfg(target_os = "linux")]\n')
        self.git('add', 'src/lib.rs')
        self.assertEqual(self.check().returncode, 0)
        self.write('src/lib.rs', b'#!/bin/sh\necho synthetic\n')
        self.git('add', 'src/lib.rs')
        self.assertIn('script absent', self.check().stderr)
        self.write('src/lib.rs', b'#![cfg(target_os = "linux")]\n')
        (self.root / 'src/lib.rs').chmod(0o755)
        self.git('add', 'src/lib.rs')
        self.assertIn('script absent', self.check().stderr)

    def test_renamed_audio_and_external_symlink_are_blocked(self):
        self.write('docs/evidence.txt', b'RIFF' + b'\0' * 4 + b'WAVE')
        self.git('add', 'docs/evidence.txt')
        self.assertIn('media/archive signature', self.check().stderr)
        self.git('rm', '--cached', 'docs/evidence.txt')
        (self.root / 'docs/link').symlink_to('/outside/private-recordings')
        self.git('add', 'docs/link')
        self.assertIn('symlink', self.check().stderr)

    def test_push_checks_a_leak_deleted_by_a_later_commit(self):
        self.git('commit', '-qm', 'safe baseline')
        baseline = self.git('rev-parse', 'HEAD').decode().strip()
        self.write('user/private.json', b'{}')
        self.git('add', '-f', 'user/private.json')
        self.git('commit', '-qm', 'simulated private commit')
        self.git('rm', '-q', 'user/private.json')
        self.git('commit', '-qm', 'removed at tip')
        tip = self.git('rev-parse', 'HEAD').decode().strip()
        self.assertEqual(self.check('--revision', tip).returncode, 0)
        result = self.check('--push', stdin=f'refs/heads/main {tip} refs/heads/main {baseline}\n')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('user/private.json', result.stderr)

    def test_push_rejects_unsafe_index_even_with_no_outgoing_commits(self):
        self.write('user/private.json', b'{}')
        self.git('add', '-f', 'user/private.json')
        result = self.check('--push', stdin='')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('user/private.json', result.stderr)

    def add_reviewed_font(self):
        import hashlib
        font = b'\x72\xb5\x4a\x86' + b'\x00' * 28
        notice = b'Synthetic font notice\n'
        policy = json.loads((self.root / 'scripts/publication-policy.json').read_text())
        policy['directories'].append('assets')
        policy['root_files'].append('THIRD_PARTY.md')
        policy['binary_assets'] = {'assets/fonts/font.psf': {
            'size': len(font), 'magic_hex': '72b54a86',
            'sha256': hashlib.sha256(font).hexdigest(),
            'required_files': {'THIRD_PARTY.md': hashlib.sha256(notice).hexdigest()},
        }}
        self.write('scripts/publication-policy.json', json.dumps(policy).encode())
        self.write('assets/fonts/font.psf', font)
        self.write('THIRD_PARTY.md', notice)
        self.git('add', 'scripts/publication-policy.json', 'assets/fonts/font.psf', 'THIRD_PARTY.md')
        self.assertEqual(self.check().returncode, 0)
        return font

    def test_reviewed_font_requires_exact_bytes_and_same_tree_notice(self):
        font = self.add_reviewed_font()
        self.write('assets/fonts/font.psf', font[:-1] + b'X')
        self.git('add', 'assets/fonts/font.psf')
        self.assertIn('binary asset identity mismatch', self.check().stderr)
        self.write('assets/fonts/font.psf', font)
        self.git('add', 'assets/fonts/font.psf')
        self.git('rm', '--cached', 'THIRD_PARTY.md')
        self.assertIn('missing or changed required notice', self.check().stderr)

    def test_font_exception_does_not_allow_another_binary(self):
        font = self.add_reviewed_font()
        self.write('assets/fonts/other.psf', font)
        self.git('add', 'assets/fonts/other.psf')
        self.assertIn('binary content outside reviewed artwork', self.check().stderr)

    def test_font_exception_preserves_media_signature_rejection(self):
        import hashlib
        font = self.add_reviewed_font()
        media = b'RIFF' + font[4:]
        policy_path = self.root / 'scripts/publication-policy.json'
        policy = json.loads(policy_path.read_text())
        asset = policy['binary_assets']['assets/fonts/font.psf']
        asset.update(magic_hex='52494646', sha256=hashlib.sha256(media).hexdigest())
        self.write('scripts/publication-policy.json', json.dumps(policy).encode())
        self.write('assets/fonts/font.psf', media)
        self.git('add', 'scripts/publication-policy.json', 'assets/fonts/font.psf')
        self.assertIn('media/archive signature', self.check().stderr)


if __name__ == '__main__':
    unittest.main()
