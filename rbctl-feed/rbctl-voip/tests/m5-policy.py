"""Exercise the installed-target ucode interpreter: m5-policy.py STAGING_DIR."""
from pathlib import Path
import subprocess
import sys
import tempfile

staging = Path(sys.argv[1]).resolve()
root = staging / 'root-mediatek'
files = Path(__file__).resolve().parents[1] / 'files'
q = ['qemu-aarch64', '-L', root, '-E', f'LD_LIBRARY_PATH={staging}/usr/lib:{root}/usr/lib',
     root/'usr/bin/ucode', '-L', str(root/'usr/lib/ucode/*.so')]
base = (files/'voip.config').read_text()
valid = base.replace("enabled '0'", "enabled '1'").replace("user ''", "user '123'").replace(
    "domain ''", "domain 'pbx.example'\n\toption password 'private-canary'")
with tempfile.TemporaryDirectory() as tmp:
    d = Path(tmp); out = d/'out'; out.mkdir()
    def run(script, *args, good=True):
        r = subprocess.run([*q, files/script, *args], capture_output=True, text=True)
        assert 'private-canary' not in r.stderr+r.stdout
        assert (r.returncode == 0) == good, r.stderr
        return r.stderr

    def render(text, good=True):
        (d/'voip').write_text(text)
        return run('render.uc', out, d, good=good)

    for line in ['', "option account 'deleted'"]:
        assert 'skipping port 0' in render(valid.replace("option account 'acc1'", line, 1))
        assert 'rbctl_voip_account0' not in (out/'config').read_text()
        assert 'rbctl_voip_account1' in (out/'config').read_text()
    assert 'skipping port' in render(base)
    for field in ["user '123'", "domain 'pbx.example'"]:
        assert 'incomplete enabled account' in render(valid.replace('option '+field, ''))
        assert not (out/'accounts').read_text()
    assert 'invalid user' in render(valid.replace("user '123'", "user 'bad user'"), False)
    legacy = "\n\toption digitmap '1xx'\n\toption incoming 'hunt'\n\toption require_registration '1'\n\toption interdigit_timeout '8'\n\toption interdigit_long_timeout '9'\n\tlist alert_info 'internal=2'"
    old = valid.replace("option account 'acc1'", "option account 'acc1'" + legacy)
    assert 'deprecated' in render(old)
    before = (out/'config').read_bytes()
    run('migrate.uc', d)
    assert not run('render.uc', out, d)
    assert before == (out/'config').read_bytes()
    render(old.replace("'1xx'", "'2xx'", 1))
    assert 'conflicting line digitmap' in run('migrate.uc', d)
    assert not run('render.uc', out, d)
    cfg = (out/'config').read_text()
    assert all(f'rbctl_voip_digitmap{i} 2xx' in cfg for i in [0, 1])
    migrated = (d/'voip').read_bytes()
    assert not run('migrate.uc', d) and migrated == (d/'voip').read_bytes()
print('PASS: skipped lines/accounts, malformed identity, secret redaction, policy migration and conflict precedence')
