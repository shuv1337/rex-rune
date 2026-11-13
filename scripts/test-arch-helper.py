#!/usr/bin/env python3
"""The pacman package's root update helper (resources/pacman/rex-rune-update).

Everywhere, as an ordinary user and in an unprivileged archlinux:latest container:
  - the polkit action as the file declares it;
  - the helper's reading of gpg's status report, fed crafted reports;
  - real packages built with the helper in them, one installed, its files and modes, and
    that without systemd the helper refuses and changes nothing.

Only on a machine that is thrown away afterwards (GitHub Actions, or a disposable VM with
REX_HELPER_TEST_VM=disposable): the helper does its work as a systemd service, so the rest
needs systemd as PID 1, and a container that boots systemd must be --privileged. Such a
container shares the machine's /dev and /sys: on a workstation it restarts the console and
ends the desktop session. **Never force this part on a machine someone uses.** There it
runs the shipped helper, unmodified, as root against a private HTTPS copy of the package
site (pkg.shuv.dev resolved to the container itself, behind a throwaway certificate
authority): every refusal the helper promises, an upgrade from a hostile environment,
terminal and resource limit, and an upgrade through pkexec whose caller is killed
mid-download. Root needs no polkit dialog, so no agent and no password is involved; what
an agent's dialog looks like is not tested.

Only throwaway keys and stub executables: no production secret, release, server session or
host package install, and nothing runs as root outside a container. Requires Docker, gpg,
openssl, zstd.
"""
import os
from pathlib import Path
import importlib.util
import re
import shutil
import subprocess
import sys
import tempfile
import time
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parent.parent
SCRIPTS = ROOT / 'scripts'
HELPER = ROOT / 'resources/pacman/rex-rune-update'


def load(name):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / f'{name}.py')
    module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
    return module


repo = load('test-arch-repo')
arch = repo.arch


def run(*args, **kw):
    return subprocess.check_output(args, **kw)


def disposable():
    """Whether this machine may boot systemd in a privileged container: one of GitHub's
    own runners, or a VM someone has said is disposable. Not `act`, which says it is
    GitHub Actions while using this machine's Docker, and not a self-hosted runner."""
    if os.environ.get('REX_HELPER_TEST_VM') == 'disposable':
        return True
    return (os.environ.get('GITHUB_ACTIONS') == 'true' and os.environ.get('RUNNER_ENVIRONMENT') == 'github-hosted'
            and 'ACT' not in os.environ)


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


def verdicts():
    """The helper's reading of gpg's report, as an ordinary user: the function as shipped,
    fed reports a container cannot easily produce."""
    function = re.search(r'^verdict\(\) \{\n.*?^\}\n', HELPER.read_text(), re.S | re.M).group(0)
    primary, subkey, other = 'A' * 40, 'B' * 40, 'C' * 40
    def valid(signer, owner):
        return f'[GNUPG:] VALIDSIG {signer} 2026-11-12 1794501064 0 4 0 22 10 00 {owner}'
    good = ['[GNUPG:] NEWSIG', f'[GNUPG:] KEY_CONSIDERED {primary} 0', f'[GNUPG:] GOODSIG {subkey[-16:]} Rex <rex@example.invalid>',
            valid(subkey, primary)]
    cases = [
        ('a good signature by a subkey of the pinned primary', good, True),
        ('with trust lines after it', good + ['[GNUPG:] TRUST_FULLY 0 pgp'], True),
        ('signed by the primary key itself', good[:3] + [valid(primary, primary)], False),
        ('a subkey of another primary', good[:3] + [valid(subkey, other)], False),
        ('two signatures, both good', good + good, False),
        ('a good signature and then a bad one', good + ['[GNUPG:] NEWSIG', f'[GNUPG:] BADSIG {other[-16:]} x'], False),
        ('good, but the signing key is revoked', good[:2] + [f'[GNUPG:] REVKEYSIG {subkey[-16:]} Rex', valid(subkey, primary)], False),
        ('good, but the signing key has expired', good[:2] + [f'[GNUPG:] EXPKEYSIG {subkey[-16:]} Rex', valid(subkey, primary)], False),
        ('good, but the signature has expired', good[:2] + [f'[GNUPG:] EXPSIG {subkey[-16:]} Rex', valid(subkey, primary)], False),
        ('a good signature and a revoked-key word beside it', good + [f'[GNUPG:] REVKEYSIG {subkey[-16:]} Rex'], False),
        ('valid but never said to be good', [good[0], good[3]], False),
        ('good but never said to be valid', good[:3], False),
        ('an unknown key', ['[GNUPG:] NEWSIG', f'[GNUPG:] ERRSIG {subkey[-16:]} 22 10 00 1794501064 9 {subkey}', f'[GNUPG:] NO_PUBKEY {subkey[-16:]}'], False),
        ('not a signature at all', ['[GNUPG:] NODATA 1', '[GNUPG:] FAILURE gpgv-exit 33554433'], False),
        ('nothing', [], False),
        ('a fingerprint that is not one', good[:3] + [f'[GNUPG:] VALIDSIG {subkey[:39]}; 2026-11-12 0 0 4 0 22 10 00 {primary}'], False),
        ('the words without gpg\'s prefix', [line.replace('[GNUPG:] ', '') for line in good], False),
    ]
    for what, lines, accepted in cases:
        result = subprocess.run(['bash', '-c', f'set -u\nPRIMARY={primary}\nsigner=\n{function}\nverdict "$REPORT" && [[ $signer == {subkey} ]]'],
                                env={'PATH': '/usr/bin:/bin', 'REPORT': '\n'.join(lines)})
        assert (result.returncode == 0) == accepted, f'{what}: {"accepted" if result.returncode == 0 else "refused"}'
    print(f'PASS: the helper\'s reading of gpg\'s report: {sum(c[2] for c in cases)} accepted, {sum(not c[2] for c in cases)} refused')


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
unit=rex-rune-update.service
fail() { echo "FAIL: $*"; journalctl -t rex-rune-update -n 6 --no-pager -o cat 2>/dev/null | sed 's/^/    journal: /'; exit 1; }

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
now=99.1.0
[ "$(rex-rune --version)" = "rex-rune $now" ] || fail 'not 99.1.0'
[ "$(stat -c '%U:%G %a' $helper)" = 'root:root 755' ] || fail "helper mode: $(stat -c '%U:%G %a' $helper)"
[ ! -u $helper ] && [ ! -g $helper ] || fail 'the helper must not be setuid'
[ "$(stat -c '%U:%G %a' /usr/share/polkit-1/actions/dev.shuv.rex-rune.update.policy)" = 'root:root 644' ] || fail 'policy mode'
[ "$(stat -c '%U:%G %a' /usr/share/rex-rune/archive-key.gpg)" = 'root:root 644' ] || fail 'certificate mode'
grep -qx "readonly PRIMARY='$primary'" $helper || fail 'the primary key was not written into the helper'
! grep -q '@PRIMARY@' $helper || fail 'placeholder left in the helper'
unchanged() {
    [ "$(pacman -Q rex-rune-bin)" = "rex-rune-bin $now-1" ] || fail "$1: the package changed: $(pacman -Q rex-rune-bin)"
    [ "$(rex-rune --version)" = "rex-rune $now" ] || fail "$1: the binary changed"
}
'''

# Anywhere: what the package installs, and that with no systemd there is no update.
PACKAGE = SETUP + r'''
[ ! -d /run/systemd/system ] || fail 'expected a container without systemd'
echo 'PASS: the package installs the helper (root, 755, not setuid), the action and the certificate, with the primary key written in'
code() { "$helper" "$@" </dev/null >/tmp/out 2>&1; echo $?; }
[ "$(code 99.1.1)" = 74 ] || fail 'without systemd: expected 74'
[ "$(code --in-service 99.1.1)" = 74 ] || fail 'without systemd, claiming to be the service: expected 74'
[ "$(code latest)" = 64 ] || fail 'not a version'
[ "$(code)" = 64 ] || fail 'no argument'
[ ! -s /tmp/out ] || fail 'the helper wrote to its caller'
runuser -u nobody -- "$helper" 99.1.1 </dev/null >/dev/null 2>&1; [ $? = 64 ] || fail 'not root'
unchanged 'without systemd'
[ ! -e /run/rex-rune-update ] && [ ! -e /var/lib/pacman/db.lck ] || fail 'something was left behind'
kill $server 2>/dev/null
echo 'PASS: without systemd the helper refuses (74) and changes nothing; bad arguments and non-root -> 64'
'''

# Only on a machine that is thrown away: under systemd, as the client will run it.
MACHINE = SETUP + r'''
[ -d /run/systemd/system ] || fail 'expected systemd in this container'
running() { case $(systemctl is-active $unit 2>/dev/null) in active|activating|deactivating|reloading) return 0;; esac; return 1; }
# After any run: no service, no work directory, no pacman lock.
settled() {
    for _ in $(seq 50); do running || [ -e /run/rex-rune-update ] || break; sleep .1; done
    ! running || fail "$1: the update service is still there"
    [ ! -e /run/rex-rune-update ] || fail "$1: work directory left behind"
    [ ! -e /var/lib/pacman/db.lck ] || fail "$1: pacman's lock left behind"
}

# expect <code> <site> <what> <version> [more arguments]: the helper's exit code, and that a
# refusal left the package as it was.
expect() {
    local want=$1 site=$2 what=$3; shift 3
    ln -sfn "/fixtures/sites/$site" /srv/site
    "$helper" "$@" </dev/null >/tmp/out 2>&1; local got=$?
    [ "$got" = "$want" ] || fail "$what: exit $got, expected $want"
    [ ! -s /tmp/out ] || fail "$what: the helper wrote to its caller: $(head -c 200 /tmp/out)"
    settled "$what"
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
[ ! -e /tmp/pwned ] || fail 'shell text ran'
runuser -u nobody -- "$helper" 99.1.1 </dev/null >/dev/null 2>&1; [ $? = 64 ] || fail 'not root'
echo 'PASS: someone who is not root -> 64'

# Claiming to be the service: from where the caller is, and from a control group of the
# same name that is not the system's.
expect 64 good 'claiming to be the service from outside it' --in-service 99.1.1
imitation=/sys/fs/cgroup/user.slice/user-1000.slice/rex-rune-update.service
mkdir -p $imitation || fail 'could not make the imitation control group'
( echo $BASHPID > $imitation/cgroup.procs || exit 99
  grep -q '/rex-rune-update\.service$' /proc/self/cgroup || exit 98
  exec "$helper" --in-service 99.1.1 ) </dev/null >/dev/null 2>&1; got=$?
[ "$got" = 64 ] || fail "from a control group of the same name under user.slice: exit $got, expected 64"
rmdir $imitation 2>/dev/null
unchanged 'an imitation control group'
echo 'PASS: claiming to be the service from a same-named control group under user.slice -> 64'

# An administrator's off-switch, and a work directory that is not one.
systemctl mask --runtime $unit >/dev/null 2>&1 || fail 'could not mask the unit'
expect 74 good 'the unit masked by an administrator' 99.1.1
systemctl unmask --runtime $unit >/dev/null 2>&1 || fail 'could not unmask the unit'
touch /run/rex-rune-update
ln -sfn /fixtures/sites/good /srv/site
"$helper" 99.1.1 </dev/null >/dev/null 2>&1; got=$?
rm -rf /run/rex-rune-update
[ "$got" = 74 ] || fail "a file where the work directory goes: exit $got, expected 74"
unchanged 'a file where the work directory goes'
echo 'PASS: a file where the work directory goes -> 74'

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
expect 69 twice 'two Rex signatures in one file' 99.1.1
expect 71 older 'an older Rex-signed package served under the newer name' 99.1.1
expect 71 wrongarch 'the Rex-signed package for another architecture under this name' 99.1.1
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
[ "$(status twice | grep -c '^\[GNUPG:\] GOODSIG ')" = 2 ] || fail 'the two-signature case should hold two good Rex signatures'
echo "PASS: the other key is unknown to the package's certificate; the to-be-revoked subkey is still good in it; the double signature is two good ones"

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
unchanged 'pacman database locked'
echo 'PASS: pacman database locked -> 67'
# A machine whose pacman keyring is somewhere else: that one is asked, not the usual place.
mkdir -m 700 /srv/empty-keyring
sed -i 's|^\[options\]$|[options]\nGPGDir = /srv/empty-keyring|' /etc/pacman.conf
expect 70 good 'a pacman keyring elsewhere that does not have the Rex key' 99.1.1
cp /tmp/pacman.conf /etc/pacman.conf
# A subkey the package's own certificate still shows as good, revoked since: the pacman
# keyring knows, and that is enough.
pacman-key --add /fixtures/keys/rex-revoked.asc >/dev/null 2>&1 || fail 'import revocation'
expect 70 revoked 'a signature by a Rex subkey this machine knows was revoked' 99.1.1
pacman-key --delete "$primary" >/dev/null 2>&1 || fail 'delete key'
expect 70 good 'the Rex key removed from this machine' 99.1.1
pacman-key --add /fixtures/keys/rex-revoked.asc >/dev/null 2>&1 || fail 're-add key'
expect 70 good 'the Rex key present but not trusted' 99.1.1
pacman-key --lsign-key "$primary" >/dev/null 2>&1 || fail 'lsign'

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
settled 'an unprivileged caller'; unchanged 'an unprivileged caller'
echo 'PASS: an unprivileged caller with nobody to authorize it -> pkexec 127, nothing changed'

# Root needs no dialog, so from here pkexec runs the helper as the client would have it run.
via() { pkexec --disable-internal-agent "$helper" "$@" </dev/null >/tmp/out 2>&1; }
via latest; [ $? = 64 ] || fail 'through pkexec: not a version'
via 99.1.0; code=$?; [ "$code" = 65 ] || fail "through pkexec and the service: exit $code, expected 65"
[ ! -s /tmp/out ] || fail 'the helper wrote to its caller'
settled 'refusals through pkexec'; unchanged 'refusals through pkexec'
echo "PASS: the helper's own exit codes come back through the service and pkexec (64, 65)"

# The first upgrade, from everything a caller might arrange: its own programs first on
# PATH, shell start-up files, a home with a curl configuration, proxies, a terminal of its
# own, and a file-size limit smaller than a file in the package. The machine also asks
# for a signature on every local package file.
sed -i 's/^\[options\]$/[options]\nLocalFileSigLevel = Required/' /etc/pacman.conf
[ "$(pacman-conf LocalFileSigLevel | head -n 1)" = PackageRequired ] || fail 'LocalFileSigLevel was not set'
# (Not under /tmp: the container's is mounted noexec, and these must be runnable to prove anything.)
mkdir -p /srv/evil
printf '#!/bin/sh\ntouch /srv/evil/ran\n' > /srv/evil/pacman; chmod 755 /srv/evil/pacman
for name in curl gpgv gpg systemd-run systemctl bsdtar; do cp /srv/evil/pacman /srv/evil/$name; done
printf 'touch /srv/evil/sourced\n' > /srv/evil/env.sh
printf 'insecure\nproxy = http://127.0.0.1:9\n' > /srv/evil/.curlrc
cat > /srv/evil/caller.sh <<CALLER
#!/bin/bash
ulimit -f 1024
export PATH="/srv/evil:\$PATH" BASH_ENV=/srv/evil/env.sh ENV=/srv/evil/env.sh HOME=/srv/evil CURL_HOME=/srv/evil
export GNUPGHOME=/srv/evil https_proxy=http://127.0.0.1:9 HTTPS_PROXY=http://127.0.0.1:9 ALL_PROXY=http://127.0.0.1:9
export IFS=. LD_LIBRARY_PATH=/srv/evil
exec /usr/bin/env "$helper" 99.1.1
CALLER
chmod 755 /srv/evil/caller.sh
ln -sfn /fixtures/sites/good /srv/site
/usr/bin/script -qec /srv/evil/caller.sh /dev/null </dev/null >/tmp/out 2>&1; got=$?
[ "$got" = 0 ] || fail "the upgrade from a hostile caller: exit $got: $(tr '\n' ' ' < /tmp/out | cut -c1-300)"
[ ! -e /srv/evil/ran ] && [ ! -e /srv/evil/sourced ] || fail "the caller's environment was used"
now=99.1.1
unchanged 'after the first upgrade'
[ "$(stat -c %s /usr/share/rex-rune/ballast)" = 4194304 ] || fail "the caller's file-size limit cut a file short: $(stat -c %s /usr/share/rex-rune/ballast)"
settled 'the first upgrade'
echo 'PASS: 99.1.0 -> 99.1.1 from a hostile environment, terminal and 1 MiB file-size limit, with LocalFileSigLevel = Required -> 0'
expect 65 good 'the same version again' 99.1.1

# The second upgrade, through pkexec, slowly, so that it can be looked at while it runs.
ln -sfn /fixtures/sites/slow /srv/site
printf '#!/bin/bash\nulimit -f 1024\nexec pkexec --disable-internal-agent %s 99.1.3\n' "$helper" > /srv/evil/pkexec.sh; chmod 755 /srv/evil/pkexec.sh
/usr/bin/script -qec /srv/evil/pkexec.sh /dev/null </dev/null >/dev/null 2>&1 & caller=$!
for _ in $(seq 100); do running && break; sleep .1; done
running || fail 'the update did not start its service'
main=$(systemctl show --property MainPID --value $unit)
[ -n "$main" ] && [ "$main" != 0 ] || fail 'the service has no main process'
[ "$(cat /proc/$main/cgroup | grep '^0::')" = "0::/system.slice/$unit" ] || fail "the work is in $(cat /proc/$main/cgroup | tr '\n' ' ')"
read -r _ _ _ parent _ session terminal _ < /proc/$main/stat
[ "$parent" = 1 ] || fail "the work is a child of $parent, not of systemd"
[ "$session" = "$main" ] || fail 'the work is not in a session of its own'
[ "$terminal" = 0 ] || fail 'the work has a controlling terminal'
grep -Eq '^Max file size +unlimited +unlimited' /proc/$main/limits || fail "the work has the caller's file-size limit: $(grep '^Max file size' /proc/$main/limits)"
! tr '\0' '\n' < /proc/$main/environ | grep -q evil || fail "the work has the caller's environment"
echo "PASS: the work is systemd's own service: its child, its own session, no terminal, no limit or environment of the caller's"
via 99.1.3; code=$?; [ "$code" = 67 ] || fail "a second update while one runs: exit $code, expected 67"
running || fail 'the second update disturbed the first'
echo 'PASS: a second update while one runs -> 67'
# The caller goes away in the middle: the client was closed, or the session ended.
kill -KILL $caller; wait $caller 2>/dev/null
running || fail 'killing the caller ended the update'
for _ in $(seq 600); do running || break; sleep .1; done
! running || fail 'the update never finished'
now=99.1.3
unchanged 'after the second upgrade'
[ "$(stat -c %s /usr/share/rex-rune/ballast)" = 4194304 ] || fail "the caller's file-size limit cut a file short"
settled 'the second upgrade'
echo 'PASS: with its caller killed mid-download, the update still finished: 99.1.1 -> 99.1.3'
journalctl -t rex-rune-update --no-pager -o cat 2>/dev/null | grep -q "^installed rex-rune-bin 99.1.3-1\$" \
    || fail 'the journal does not say what was installed'
journalctl -t rex-rune-update --no-pager -o cat 2>/dev/null | grep -q '^refused (69): ' || fail 'the journal does not say what was refused'
echo 'PASS: the journal says what was refused and what was installed'
via 99.1.3; [ $? = 65 ] || fail 'a second run'
kill $server 2>/dev/null
echo 'PASS: update helper under systemd: every refusal, an upgrade from a hostile caller, an upgrade through pkexec'
'''


def main():
    policy()
    verdicts()
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

            # Packages: 99.1.0 to start from; 99.1.1 and 99.1.3 to upgrade to, each with a
            # 4 MiB file in it; and 99.1.2, which needs a package the container does not have.
            releases = root / 'releases'; releases.mkdir()
            plain = (ROOT / 'packaging/arch/PKGBUILD.in').read_text()
            needy = plain.replace("'hicolor-icon-theme')", "'hicolor-icon-theme' 'tree')")
            assert "'tree')" in needy
            marker = '    install -dm755 "$pkgdir/usr/share/rex-rune"\n'
            assert plain.count(marker) == 1
            heavy = plain.replace(marker, marker + '    head -c 4194304 /dev/zero > "$pkgdir/usr/share/rex-rune/ballast"\n')
            for version, recipe in (('99.1.0', None), ('99.1.1', heavy), ('99.1.2', needy), ('99.1.3', heavy)):
                repo.fixture(releases / version, version, 'rex-rune', certificate, recipe)

            def package(version):
                return releases / version / f'rex-rune-bin-{version}-1-x86_64.pkg.tar.zst'

            # The expired subkey signs as of an hour into its one day.
            by_expiring = root / 'expiring.sig'
            sign(expiring, package('99.1.1'), by_expiring, '--faked-system-time', str(past + 3600))
            by_doomed = root / 'doomed.sig'; sign(doomed, package('99.1.1'), by_doomed)
            by_other = root / 'other.sig'; sign(other, package('99.1.1'), by_other)
            signatures = {}
            for version in ('99.1.0', '99.1.1', '99.1.2', '99.1.3'):
                signatures[version] = root / f'{version}.sig'
                sign(signing, package(version), signatures[version])
            foreign = releases / '99.1.1' / 'rex-rune-bin-99.1.1-1-aarch64.pkg.tar.zst'
            by_signing_foreign = root / 'foreign.sig'; sign(signing, foreign, by_signing_foreign)
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
            # Two good Rex signatures in one file, and the other architecture's package.
            site('twice', {**good, new + '.sig': signatures['99.1.1'].read_bytes() + by_doomed.read_bytes()})
            site('wrongarch', {new: foreign, new + '.sig': by_signing_foreign})
            (site('redirect', good).parents[1] / 'REDIRECT').touch()
            later = package('99.1.3').name
            (site('slow', {later: package('99.1.3'), later + '.sig': signatures['99.1.3']}).parents[1] / 'SLOW').touch()

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

            # No signing key and no secret goes into a container.
            subprocess.run(['docker', 'run', '--rm', '-v', f'{fixtures}:/fixtures:ro', arch.ARCH_IMAGE,
                            'bash', '-c', PACKAGE, 'bash', primary, other, ''], check=True)
            if not disposable():
                print('SKIPPED: everything the helper does under systemd (the refusals, both upgrades, pkexec).\n'
                      '    That needs systemd booted in a --privileged container, which on a workstation takes\n'
                      '    the desktop session down with it. It runs in GitHub Actions (the pacman job), or in a\n'
                      '    disposable VM with REX_HELPER_TEST_VM=disposable. Do not set that anywhere else.')
                return
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
                subprocess.run(['docker', 'exec', machine, 'bash', '-c', MACHINE, 'bash', primary, other, 'polkit'], check=True)
            finally:
                subprocess.run(['docker', 'kill', machine], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            print('PASS: the root update helper under systemd and through pkexec')
        finally:
            subprocess.run(['gpgconf', '--homedir', str(home), '--kill', 'gpg-agent'])


if __name__ == '__main__':
    main()
