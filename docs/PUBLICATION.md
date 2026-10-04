# Source publication

Source publication is authorized for this project. Keep private roles, settings,
provider executables/libraries, worker logs, recordings and generated previews
outside Git. No binary release, installer, deployment or hardware acceptance is
implied by a source push.

Enable the reviewed hooks with `git config --local core.hooksPath .githooks` after
checking existing hooks. The pre-commit guard checks the complete index. Pre-push
also checks every outgoing commit, including deleted historical leaks and the
complete history for a new ref. Inspect named staged content and run
`git diff --cached --check` and `python3 scripts/check_publication.py` before commit.
No blanket staging or guard bypass. New scripts require explicit policy review.

The independent guard is adapted from GigPies (MIT). The exact unmodified Terminus
PSF2 asset is allowed only by path, size, magic and SHA-256. Its exact same-tree OFL
and THIRD_PARTY notice are required. Other binary/media, private-path, credential,
size and symlink restrictions remain. Licence text stays unchanged; its whitespace
attribute applies only to that file. See THIRD_PARTY.md for provenance.

Run `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts -p 'test_*.py'`
for synthetic complete-index and outgoing-history checks. Run the normal software
and native-feature suites for affected production code before publication. Physical
HDMI/controller/audio/MIDI/DMX and load acceptance remain separate explicit gates.
