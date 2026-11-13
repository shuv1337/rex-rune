#!/usr/bin/env python3
"""The pacman package's root update helper, run as root in archlinux:latest.

Builds real packages with the helper in them, installs one in a container, and runs
/usr/lib/rex-rune/rex-rune-update there against a private HTTPS copy of the package site
(pkg.shuv.dev resolved to the container itself, behind a throwaway certificate authority),
so the helper under test is the shipped file with no test switch in it.

Two containers. The first has no systemd: every refusal the helper promises, and one real
upgrade under a hostile environment. The second boots systemd and installs polkit, and
runs the helper the way the client will, through pkexec: the action as polkit lists it, a
caller nobody can authorize, exit codes through the scope, the work in a system scope of
its own, a second update refused while one runs, and an update that still finishes when
its caller is killed mid-download. Root needs no dialog, so no polkit agent and no
password is involved; what an agent's dialog looks like is not tested here.

Only throwaway keys and stub executables: no production secret, release, server session
or host package install, and nothing runs as root outside the containers. Requires Docker
(the second container is --privileged, for systemd), gpg, openssl, zstd.
"""
import hashlib
import importlib.util
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parent.parent
SCRIPTS = ROOT / 'scripts'


def load(name):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / f'{name}.py')
    module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
    return module


repo = load('test-arch-repo')
arch = repo.arch


def run(*args, **kw):
    return subprocess.check_output(args, **kw)


def policy():
    """What the action allows, read from the file that ships."""
    tree = ET.parse(ROOT / 'resources/pacman/dev.shuv.rex-rune.update.policy').getroot()
    actions = tree.findall('action')
    assert len(actions) == 1 and actions[0].get('id') == 'dev.shuv.rex-rune.update'
    defaults = {child.tag: child.text for child in actions[0].find('defaults')}
    # A password every time, for a local active session only; never a standing yes.
    assert defaults == {'allow_any': 'no', 'allow_inactive': 'no', 'allow_active': 'auth_admin'}, defaults
    annotations = {a.get('key'): a.text for a in actions[0].findall('annotate')}
    assert annotations == {'org.freedesktop.policykit.exec.path': '/usr/lib/rex-rune/rex-rune-update'}, annotations
    message = actions[0].find('message').text
    assert 'Rex Rune' in message and '$(' not in message, 'the dialog names Rex and echoes no argument'
    dtd = Path('/usr/share/polkit-1/policyconfig-1.dtd')
    if dtd.is_file() and shutil.which('xmllint'):
        subprocess.run(['xmllint', '--noout', '--nonet', '--dtdvalid', str(dtd),
                        str(ROOT / 'resources/pacman/dev.shuv.rex-rune.update.policy')],
                       check=True, stderr=subprocess.DEVNULL)
        print('PASS: policy validates against polkit\'s DTD')
    print('PASS: one action, auth_admin for an active local session, bound to the helper')


SERVER = r'''
import functools, http.server, os, ssl, time
class Handler(http.server.SimpleHTTPRequestHandler):
    def do_GET(self):
        # A case that needs an update to still be running a few seconds later.
        if os.path.exists('/srv/site/SLOW') and self.path.endswith('.zst'):
            time.sleep(8)
        # A case that must show the helper follows no redirect.
        if os.path.exists('/srv/site/REDIRECT'):
            self.send_response(302)
            self.send_header('Location', 'https://pkg.shuv.dev/elsewhere' + self.path)
            self.end_headers()
            self.wfile.write(b'moved')
            return
        super().do_GET()
    def log_message(self, *args): pass
# /srv/site is a link the test points at one case after another: resolve it per request.
server = http.server.ThreadingHTTPServer(('127.0.0.1', 443), functools.partial(Handler, directory='/srv/site'))
context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.load_cert_chain('/fixtures/tls/server.pem', '/fixtures/tls/server.key')
server.socket = context.wrap_socket(server.socket, server_side=True)
open('/tmp/server.ready', 'w').close()
server.serve_forever()
'''

# Both containers start the same way: this machine's pacman keyring trusts the fixture Rex
# key (and another packager's), a private pkg.shuv.dev is up, and 99.1.0 is installed.
SETUP = r'''
set -u
primary=$1 other=$2 extra=$3
helper=/usr/lib/rex-rune/rex-rune-update
fail() { echo "FAIL: $*"; exit 1; }

pacman-key --init >/dev/null 2>&1
pacman-key --populate archlinux >/dev/null 2>&1
# shellcheck disable=SC2086
pacman -Syu --noconfirm python $extra >/dev/null 2>&1 || fail 'could not install what the test needs'
rex_key() { pacman-key --add "$1" >/dev/null 2>&1 && pacman-key --lsign-key "$primary" >/dev/null 2>&1; }
rex_key /fixtures/keys/rex.asc || fail 'rex key'
# Another packager this machine trusts: its signature must not be enough.
pacman-key --add /fixtures/keys/other.asc >/dev/null 2>&1 && pacman-key --lsign-key "$other" >/dev/null 2>&1 || fail 'other key'

trust anchor /fixtures/tls/ca.pem || fail 'trust anchor'
update-ca-trust
echo '127.0.0.1 pkg.shuv.dev' >> /etc/hosts
mkdir -p /srv
ln -sfn /fixtures/sites/good /srv/site
python /fixtures/server.py & server=$!
for _ in $(seq 100); do [ -e /tmp/server.ready ] && break; sleep .1; done
[ -e /tmp/server.ready ] || fail 'the fixture server did not start'
curl -fsS -o /dev/null https://pkg.shuv.dev/arch/x86_64/rex-rune-bin-99.1.1-1-x86_64.pkg.tar.zst || fail 'the fixture site is not reachable as pkg.shuv.dev'
# The cases really are different sites: this one has no signature file.
ln -sfn /fixtures/sites/nosig /srv/site
! curl -fsS -o /dev/null https://pkg.shuv.dev/arch/x86_64/rex-rune-bin-99.1.1-1-x86_64.pkg.tar.zst.sig 2>/dev/null || fail 'the fixture server does not follow the site link'

# The starting point: 99.1.0 installed from a signed file, bringing the helper with it.
pacman -U --noconfirm /fixtures/packages/rex-rune-bin-99.1.0-1-x86_64.pkg.tar.zst >/dev/null 2>&1 || fail 'install 99.1.0'
[ "$(rex-rune --version)" = 'rex-rune 99.1.0' ] || fail 'not 99.1.0'
[ "$(stat -c '%U:%G %a' $helper)" = 'root:root 755' ] || fail "helper mode: $(stat -c '%U:%G %a' $helper)"
[ ! -u $helper ] && [ ! -g $helper ] || fail 'the helper must not be setuid'
[ "$(stat -c '%U:%G %a' /usr/share/polkit-1/actions/dev.shuv.rex-rune.update.policy)" = 'root:root 644' ] || fail 'policy mode'
[ "$(stat -c '%U:%G %a' /usr/share/rex-rune/archive-key.gpg)" = 'root:root 644' ] || fail 'certificate mode'
grep -qx "readonly PRIMARY='$primary'" $helper || fail 'the primary key was not written into the helper'
! grep -q '@PRIMARY@' $helper || fail 'placeholder left in the helper'
unchanged() {
    [ "$(pacman -Q rex-rune-bin)" = 'rex-rune-bin 99.1.0-1' ] || fail "$1: the package changed: $(pacman -Q rex-rune-bin)"
    [ "$(rex-rune --version)" = 'rex-rune 99.1.0' ] || fail "$1: the binary changed"
}
'''

CONSUMER = SETUP + r'''
[ ! -d /run/systemd/system ] || fail 'expected a container without systemd'

# expect <code> <site> <what> <version> [more arguments]: the helper's exit code, and that a
# refusal left the package, the work directory and pacman's lock as they were.
expect() {
    local want=$1 site=$2 what=$3; shift 3
    ln -sfn "/fixtures/sites/$site" /srv/site
    "$helper" "$@" </dev/null >/tmp/out 2>&1; local got=$?
    [ "$got" = "$want" ] || fail "$what: exit $got, expected $want"
    [ ! -s /tmp/out ] || fail "$what: the helper wrote to its caller: $(head -c 200 /tmp/out)"
    [ -z "$(find /run -maxdepth 1 -name 'rex-rune-update.*' ! -name '*.lock')" ] || fail "$what: work directory left behind"
    [ ! -e /var/lib/pacman/db.lck ] || fail "$what: pacman's lock left behind"
    [ "$want" = 0 ] || unchanged "$what"
    echo "PASS: $what -> $want"
}

# Arguments: one version and nothing else.
"$helper" </dev/null >/dev/null 2>&1; [ $? = 64 ] || fail 'no argument'
expect 64 good 'not a version' latest
expect 64 good 'a path' ../../etc/passwd
expect 64 good 'an option' --help
expect 64 good 'a version with a release number' 99.1.1-1
expect 64 good 'shell text' '99.1.1;touch /tmp/pwned'
expect 64 good 'two arguments' 99.1.1 99.1.2
expect 64 good 'a leading zero' 099.1.1
expect 64 good 'claiming to be in the scope, with no systemd' --in-scope 99.1.1
[ ! -e /tmp/pwned ] || fail 'shell text ran'
runuser -u nobody -- "$helper" 99.1.1 </dev/null >/dev/null 2>&1; [ $? = 64 ] || fail 'not root'
echo 'PASS: someone who is not root -> 64'

# Only an upgrade.
expect 65 good 'the installed version' 99.1.0
expect 65 good 'an older version' 99.0.9

# What the site serves.
expect 68 good 'a version the site does not have' 99.9.9
expect 68 redirect 'a redirect is not followed' 99.1.1
expect 68 nosig 'a package with no signature beside it' 99.1.1
expect 69 other 'a package signed by another key this machine trusts' 99.1.1
expect 69 tampered 'a package whose bytes were changed' 99.1.1
expect 69 badsig 'a signature that was changed' 99.1.1
expect 69 expired 'a signature by a Rex subkey that has since expired' 99.1.1
expect 71 older 'an older Rex-signed package served under the newer name' 99.1.1
expect 72 depends 'a package that would pull in another' 99.1.2

# Why those were refused, shown with the same tools: gpgv is happy to exit 0 for a signature
# by an expired key, which is why the helper reads its report and not its exit code.
status() { gpgv --keyring /usr/share/rex-rune/archive-key.gpg --status-fd 1 "/fixtures/sites/$1/arch/x86_64/$file.sig" "/fixtures/sites/$1/arch/x86_64/$file" 2>/dev/null; }
file=rex-rune-bin-99.1.1-1-x86_64.pkg.tar.zst
status expired >/tmp/status; code=$?
[ "$code" = 0 ] && grep -q '^\[GNUPG:\] EXPKEYSIG ' /tmp/status && grep -q '^\[GNUPG:\] VALIDSIG ' /tmp/status \
    || fail "the expired case is not what it should be: gpgv exit $code, $(tr '\n' ' ' < /tmp/status | cut -c1-300)"
echo 'PASS: gpgv itself exits 0 and says VALIDSIG for the expired subkey; only EXPKEYSIG tells'
status good | grep -q "^\[GNUPG:\] VALIDSIG .* $primary\$" || fail 'the good signature does not name the primary'
status other | grep -Eq '^\[GNUPG:\] (NO_PUBKEY|ERRSIG) ' || fail 'the other key was known to the pinned keyring'
status revoked | grep -q '^\[GNUPG:\] GOODSIG ' || fail 'the revoked case should pass the pinned certificate'
echo "PASS: the other key is unknown to the package's certificate; the to-be-revoked subkey is still good in it"

# This machine's say: held back, locked, and no longer trusted.
cp /etc/pacman.conf /tmp/pacman.conf
sed -i 's/^\[options\]$/[options]\nIgnorePkg = rex-rune-bin/' /etc/pacman.conf
expect 66 good 'held back by IgnorePkg' 99.1.1
cp /tmp/pacman.conf /etc/pacman.conf
sed -i 's/^\[options\]$/[options]\nIgnorePkg = linux rex-*/' /etc/pacman.conf
expect 66 good 'held back by an IgnorePkg pattern' 99.1.1
cp /tmp/pacman.conf /etc/pacman.conf
touch /var/lib/pacman/db.lck
ln -sfn /fixtures/sites/good /srv/site
"$helper" 99.1.1 </dev/null >/dev/null 2>&1; [ $? = 67 ] || fail 'database lock'
rm -f /var/lib/pacman/db.lck
echo 'PASS: pacman database locked -> 67'
( flock 9; sleep 20 ) 9>>/run/rex-rune-update.lock & holder=$!
sleep .5
expect 67 good 'another update running' 99.1.1
kill $holder 2>/dev/null; wait $holder 2>/dev/null
# A subkey the package's own certificate still shows as good, revoked since: the pacman
# keyring knows, and that is enough.
pacman-key --add /fixtures/keys/rex-revoked.asc >/dev/null 2>&1 || fail 'import revocation'
expect 70 revoked 'a signature by a Rex subkey this machine knows was revoked' 99.1.1
pacman-key --delete "$primary" >/dev/null 2>&1 || fail 'delete key'
expect 70 good 'the Rex key removed from this machine' 99.1.1
pacman-key --add /fixtures/keys/rex-revoked.asc >/dev/null 2>&1 || fail 're-add key'
expect 70 good 'the Rex key present but not trusted' 99.1.1
pacman-key --lsign-key "$primary" >/dev/null 2>&1 || fail 'lsign'

# And the upgrade itself, with an environment a caller might wish for.
mkdir -p /tmp/evil
printf '#!/bin/sh\ntouch /tmp/evil/ran\n' > /tmp/evil/pacman; chmod 755 /tmp/evil/pacman
cp /tmp/evil/pacman /tmp/evil/curl; cp /tmp/evil/pacman /tmp/evil/gpgv
printf 'touch /tmp/evil/sourced\n' > /tmp/evil/env.sh
printf 'insecure\nproxy = http://127.0.0.1:9\n' > /tmp/evil/.curlrc
ln -sfn /fixtures/sites/good /srv/site
env PATH="/tmp/evil:$PATH" BASH_ENV=/tmp/evil/env.sh ENV=/tmp/evil/env.sh HOME=/tmp/evil CURL_HOME=/tmp/evil \
    GNUPGHOME=/tmp/evil https_proxy=http://127.0.0.1:9 HTTPS_PROXY=http://127.0.0.1:9 ALL_PROXY=http://127.0.0.1:9 \
    IFS=. LD_LIBRARY_PATH=/tmp/evil "$helper" 99.1.1 </dev/null >/tmp/out 2>&1
[ $? = 0 ] || fail "the upgrade: exit $?"
[ ! -s /tmp/out ] || fail 'the helper wrote to its caller'
[ ! -e /tmp/evil/ran ] && [ ! -e /tmp/evil/sourced ] || fail "the caller's environment was used"
[ "$(pacman -Q rex-rune-bin)" = 'rex-rune-bin 99.1.1-1' ] || fail "after the upgrade: $(pacman -Q rex-rune-bin)"
[ "$(rex-rune --version)" = 'rex-rune 99.1.1' ] || fail 'the binary is not 99.1.1'
[ -z "$(find /run -maxdepth 1 -name 'rex-rune-update.*' ! -name '*.lock')" ] || fail 'work directory left behind'
[ ! -e /var/lib/pacman/db.lck ] || fail "pacman's lock left behind"
echo 'PASS: 99.1.0 -> 99.1.1, with a hostile environment ignored -> 0'
# Again is not an upgrade, and the helper that came with 99.1.1 says the same.
"$helper" 99.1.1 </dev/null >/dev/null 2>&1; [ $? = 65 ] || fail 'a second run'
echo 'PASS: the same version again -> 65'
kill $server 2>/dev/null
echo 'PASS: update helper, every refusal and one upgrade'
'''

# The way it really runs: under systemd, through pkexec and polkit.
SYSTEMD = SETUP + r'''
[ -d /run/systemd/system ] || fail 'expected systemd in this container'
scope=rex-rune-update.scope
procs=/sys/fs/cgroup/system.slice/$scope/cgroup.procs

# polkit knows the action as the package declares it.
pkaction --action-id dev.shuv.rex-rune.update --verbose >/tmp/action 2>&1 || fail "polkit does not know the action: $(cat /tmp/action)"
grep -Eq '^ +implicit any: +no$' /tmp/action && grep -Eq '^ +implicit inactive: +no$' /tmp/action \
    && grep -Eq '^ +implicit active: +auth_admin$' /tmp/action \
    && grep -Eq "^ +annotation: +org.freedesktop.policykit.exec.path -> $helper\$" /tmp/action \
    || fail "the action is not what the package declares: $(tr '\n' ' ' < /tmp/action)"
echo 'PASS: polkit lists the action: administrator password, active local session only, this helper only'

# Someone with no way to answer polkit's question gets nowhere, and the helper does not run.
ln -sfn /fixtures/sites/good /srv/site
runuser -u nobody -- pkexec --disable-internal-agent "$helper" 99.1.1 </dev/null >/dev/null 2>&1; code=$?
[ "$code" = 127 ] || fail "an unprivileged caller with no agent: pkexec exit $code, expected 127"
unchanged 'an unprivileged caller'
runuser -u nobody -- "$helper" 99.1.1 </dev/null >/dev/null 2>&1; [ $? = 64 ] || fail 'not root, directly'
echo 'PASS: an unprivileged caller with nobody to authorize it -> pkexec 127, nothing changed'

# Root needs no dialog, so from here pkexec runs the helper as the client would have it run.
via() { pkexec --disable-internal-agent "$helper" "$@" </dev/null >/tmp/out 2>&1; }
via latest; [ $? = 64 ] || fail 'through pkexec: not a version'
via 99.1.0; code=$?; [ "$code" = 65 ] || fail "through pkexec and the scope: exit $code, expected 65"
[ ! -s /tmp/out ] || fail 'the helper wrote to its caller'
unchanged 'refusals through pkexec'
echo "PASS: the helper's own exit codes come back through the scope and pkexec (64, 65)"
"$helper" --in-scope 99.1.1 </dev/null >/dev/null 2>&1; [ $? = 64 ] || fail 'claimed to be in the scope from outside it'
unchanged 'a false --in-scope'
echo 'PASS: --in-scope from outside the scope -> 64'

# The upgrade, slowly, so that it can be looked at while it runs.
ln -sfn /fixtures/sites/slow /srv/site
pkexec --disable-internal-agent "$helper" 99.1.1 </dev/null >/dev/null 2>&1 & caller=$!
# (A control group's file has no size to test: read it.)
members() { cat $procs 2>/dev/null; }
for _ in $(seq 100); do [ -n "$(members)" ] && break; sleep .1; done
[ -n "$(members)" ] || fail 'the update did not start a scope of its own'
systemctl is-active --quiet $scope || fail 'the scope is not active'
[ "$(cat "/proc/$caller/comm")" = rex-rune-update ] || fail "the process pkexec started is $(cat "/proc/$caller/comm")"
! members | grep -qx "$caller" || fail 'the process pkexec started is inside the scope; expected it to stay with its caller'
grep -q "/$scope\$" "/proc/$(members | head -n 1)/cgroup" || fail 'the work is not in the scope'
echo 'PASS: the work runs in a system scope; the process pkexec started stays outside and waits'
via 99.1.1; code=$?; [ "$code" = 67 ] || fail "a second update while one runs: exit $code, expected 67"
echo 'PASS: a second update while one runs -> 67'
# The caller goes away in the middle: the client was closed, or the session ended.
kill -KILL $caller; wait $caller 2>/dev/null
[ ! -e "/proc/$caller" ] || fail 'the caller is still there'
[ -n "$(members)" ] || fail 'killing the caller ended the update'
for _ in $(seq 600); do systemctl is-active --quiet $scope || break; sleep .1; done
! systemctl is-active --quiet $scope || fail 'the update never finished'
[ "$(pacman -Q rex-rune-bin)" = 'rex-rune-bin 99.1.1-1' ] || fail "after the caller was killed: $(pacman -Q rex-rune-bin)"
[ "$(rex-rune --version)" = 'rex-rune 99.1.1' ] || fail 'the binary is not 99.1.1'
[ -z "$(find /run -maxdepth 1 -name 'rex-rune-update.*' ! -name '*.lock')" ] || fail 'work directory left behind'
[ ! -e /var/lib/pacman/db.lck ] || fail "pacman's lock left behind"
echo 'PASS: with its caller killed mid-download, the update still finished: 99.1.0 -> 99.1.1'
journalctl -t rex-rune-update --no-pager -o cat 2>/dev/null | grep -q "^installed rex-rune-bin 99.1.1-1\$" \
    || fail "the journal does not say what was installed: $(journalctl -t rex-rune-update --no-pager -o cat 2>&1 | tail -n 5 | tr '\n' ' ')"
echo 'PASS: the journal says what was refused and what was installed'
via 99.1.1; [ $? = 65 ] || fail 'a second run'
kill $server 2>/dev/null
echo 'PASS: update helper under systemd, through pkexec'
'''


def main():
    policy()
    with tempfile.TemporaryDirectory(prefix='rex-helper-test-') as temp:
        root = Path(temp); root.chmod(0o755)
        home = root / 'gpg'; home.mkdir(mode=0o700)
        env = {**os.environ, 'GNUPGHOME': str(home)}
        password = 'throwaway-helper-test-password'
        gpg = ['gpg', '--batch', '--pinentry-mode', 'loopback', '--passphrase', password]

        def fingerprints(name):
            lines = run(*gpg, '--with-colons', '--list-keys', name, env=env).decode().splitlines()
            return [l.split(':')[9] for l in lines if l.startswith('fpr:')]

        try:
            # The Rex certificate: a certify-only primary, as in production, and three signing
            # subkeys. One expired long ago and one is revoked later; the newest is the one
            # packages are signed with. The expired one is made, and later signs, with gpg
            # told it is three days ago, so nothing here waits on a clock.
            past = int(time.time()) - 3 * 86400
            then = ['--faked-system-time', str(past)]
            run(*gpg, *then, '--quick-generate-key', 'Rex helper fixture <fixture@example.invalid>', 'ed25519', 'cert', 'never', env=env)
            primary = fingerprints('fixture@example.invalid')[0]
            run(*gpg, *then, '--quick-add-key', primary, 'ed25519', 'sign', '1d', env=env)
            run(*gpg, '--quick-add-key', primary, 'ed25519', 'sign', '3y', env=env)
            run(*gpg, '--quick-add-key', primary, 'ed25519', 'sign', '3y', env=env)
            _, expiring, doomed, signing = fingerprints('fixture@example.invalid')
            run(*gpg, '--quick-generate-key', 'Another packager <other@example.invalid>', 'ed25519', 'sign', 'never', env=env)
            other = fingerprints('other@example.invalid')[0]
            keys = root / 'keys'; keys.mkdir()
            certificate = keys / 'rex.asc'
            certificate.write_bytes(run(*gpg, '--armor', '--export', primary, env=env))
            (keys / 'other.asc').write_bytes(run(*gpg, '--armor', '--export', other, env=env))

            def sign(key, path, out, *when):
                run(*gpg, *when, '--local-user', key + '!', '--detach-sign', '--output', str(out), str(path), env=env)

            # Packages: 99.1.0 to start from, 99.1.1 to upgrade to, and 99.1.2, which needs a
            # package the container does not have.
            releases = root / 'releases'; releases.mkdir()
            needy = (ROOT / 'packaging/arch/PKGBUILD.in').read_text().replace("'hicolor-icon-theme')", "'hicolor-icon-theme' 'tree')")
            assert "'tree')" in needy
            for version, recipe in (('99.1.0', None), ('99.1.1', None), ('99.1.2', needy)):
                repo.fixture(releases / version, version, 'rex-rune', certificate, recipe)

            def package(version):
                return releases / version / f'rex-rune-bin-{version}-1-x86_64.pkg.tar.zst'

            # The expired subkey signs as of an hour into its one day.
            by_expiring = root / 'expiring.sig'
            sign(expiring, package('99.1.1'), by_expiring, '--faked-system-time', str(past + 3600))
            by_doomed = root / 'doomed.sig'; sign(doomed, package('99.1.1'), by_doomed)
            by_other = root / 'other.sig'; sign(other, package('99.1.1'), by_other)
            signatures = {}
            for version in ('99.1.0', '99.1.1', '99.1.2'):
                signatures[version] = root / f'{version}.sig'
                sign(signing, package(version), signatures[version])
            # The certificate as a refreshed keyring would have it: one subkey revoked.
            index = fingerprints('fixture@example.invalid').index(doomed)
            subprocess.run([*gpg, '--command-fd', '0', '--status-fd', '2', '--edit-key', primary],
                           input=f'key {index}\nrevkey\ny\n0\n\ny\nsave\n'.encode(), env=env, check=True,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            revoked = run(*gpg, '--with-colons', '--list-keys', primary, env=env).decode()
            assert any(l.startswith('sub:r:') for l in revoked.splitlines()), 'the subkey was not revoked'
            (keys / 'rex-revoked.asc').write_bytes(run(*gpg, '--armor', '--export', primary, env=env))

            fixtures = root / 'fixtures'; fixtures.mkdir()
            shutil.copytree(keys, fixtures / 'keys')
            packages = fixtures / 'packages'; packages.mkdir()
            shutil.copy2(package('99.1.0'), packages)
            shutil.copy2(signatures['99.1.0'], packages / (package('99.1.0').name + '.sig'))

            def site(name, entries):
                directory = fixtures / 'sites' / name / 'arch/x86_64'; directory.mkdir(parents=True)
                for filename, source in entries.items():
                    if isinstance(source, bytes):
                        (directory / filename).write_bytes(source)
                    else:
                        shutil.copy2(source, directory / filename)
                return directory

            new = package('99.1.1').name
            good = {new: package('99.1.1'), new + '.sig': signatures['99.1.1']}
            site('good', good)
            site('nosig', {new: package('99.1.1')})
            site('other', {**good, new + '.sig': by_other})
            site('expired', {**good, new + '.sig': by_expiring})
            site('revoked', {**good, new + '.sig': by_doomed})
            changed = bytearray(package('99.1.1').read_bytes()); changed[len(changed) // 2] ^= 1
            site('tampered', {**good, new: bytes(changed)})
            bad = bytearray(signatures['99.1.1'].read_bytes()); bad[-1] ^= 1
            site('badsig', {**good, new + '.sig': bytes(bad)})
            # A real, Rex-signed, older package under the newer package's file name.
            site('older', {new: package('99.1.0'), new + '.sig': signatures['99.1.0']})
            site('depends', {package('99.1.2').name: package('99.1.2'),
                             package('99.1.2').name + '.sig': signatures['99.1.2']})
            (site('redirect', good).parents[1] / 'REDIRECT').touch()
            (site('slow', good).parents[1] / 'SLOW').touch()

            # pkg.shuv.dev, privately: a throwaway authority the container is told to trust.
            tls = fixtures / 'tls'; tls.mkdir()
            quiet = {'check': True, 'stdout': subprocess.DEVNULL, 'stderr': subprocess.DEVNULL}
            subprocess.run(['openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '2',
                            '-subj', '/CN=Rex helper fixture authority', '-keyout', str(root / 'ca.key'),
                            '-out', str(tls / 'ca.pem')], **quiet)
            subprocess.run(['openssl', 'req', '-newkey', 'rsa:2048', '-nodes', '-subj', '/CN=pkg.shuv.dev',
                            '-keyout', str(tls / 'server.key'), '-out', str(root / 'server.csr')], **quiet)
            (root / 'san.cnf').write_text('subjectAltName=DNS:pkg.shuv.dev\n')
            subprocess.run(['openssl', 'x509', '-req', '-in', str(root / 'server.csr'), '-CA', str(tls / 'ca.pem'),
                            '-CAkey', str(root / 'ca.key'), '-CAcreateserial', '-days', '2',
                            '-extfile', str(root / 'san.cnf'), '-out', str(tls / 'server.pem')], **quiet)
            (fixtures / 'server.py').write_text(SERVER)
            for path in fixtures.rglob('*'):
                path.chmod(0o755 if path.is_dir() else 0o644)
            fixtures.chmod(0o755)

            # No signing key and no secret goes into the containers.
            subprocess.run(['docker', 'run', '--rm', '-v', f'{fixtures}:/fixtures:ro', arch.ARCH_IMAGE,
                            'bash', '-c', CONSUMER, 'bash', primary, other, ''], check=True)
            # The second one boots systemd, which needs more of the container runtime.
            machine = run('docker', 'run', '--detach', '--rm', '--privileged', '--cgroupns=private',
                          '--tmpfs', '/run', '--tmpfs', '/tmp', '--env', 'container=docker',
                          '-v', f'{fixtures}:/fixtures:ro', arch.ARCH_IMAGE, '/usr/lib/systemd/systemd').decode().strip()
            try:
                for _ in range(120):
                    state = subprocess.run(['docker', 'exec', machine, 'systemctl', 'is-system-running'],
                                           stdout=subprocess.PIPE, stderr=subprocess.DEVNULL).stdout.decode().strip()
                    if state in ('running', 'degraded'):
                        break
                    time.sleep(.5)
                else:
                    raise SystemExit(f'FAIL: systemd did not come up in the container ({state})')
                subprocess.run(['docker', 'exec', machine, 'bash', '-c', SYSTEMD, 'bash', primary, other, 'polkit'], check=True)
            finally:
                subprocess.run(['docker', 'kill', machine], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        finally:
            subprocess.run(['gpgconf', '--homedir', str(home), '--kill', 'gpg-agent'])
    print('PASS: policy, package contents and the root update helper in a real Arch container')


if __name__ == '__main__':
    main()
