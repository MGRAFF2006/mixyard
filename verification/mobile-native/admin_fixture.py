"""Owned hosted ADMIN-domain certificates only; never change authorization policy."""
import hashlib
import json
import os
from pathlib import Path
import ssl
import stat
import sys

ABSENT = {-25300, -25263}  # Apple SecBase: item-not-found / no-trust-settings.


class OwnedAdmin:
    names = ('a', 'b', 'viewer')
    def __init__(self, fixture, run):
        if os.environ.get('GITHUB_ACTIONS') != 'true' or os.environ.get('RUNNER_OS') != 'macOS':
            raise RuntimeError('owned-host-required')
        self.fixture = Path(fixture)
        info = self.fixture.stat()
        if not self.fixture.is_absolute() or self.fixture.resolve(strict=True) != self.fixture or info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) & 0o077:
            raise RuntimeError('owned-private-fixture-required')
        self.run, self.attempted, self.admitted = run, [], False
        self.certificates = {}
        for name in self.names:
            source = self.fixture / name / 'certificate.pem'
            if source.resolve(strict=True) != source or source.stat().st_uid != os.getuid() or source.stat().st_size > 65536:
                raise RuntimeError('owned-certificate-invalid')
            data = ssl.PEM_cert_to_DER_cert(source.read_text())
            destination = self.fixture / ('owned-admin-' + name + '.der')
            # Immutable snapshot binds every add/remove/readback to the exact
            # fresh fixture fingerprint, never a later substituted certificate.
            descriptor = os.open(destination, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o400)
            with os.fdopen(descriptor, 'wb') as stream: stream.write(data)
            self.certificates[name] = (destination, hashlib.sha256(data).hexdigest())
        if len({digest for _,digest in self.certificates.values()}) != len(self.names):
            raise RuntimeError('owned-certificates-not-unique')

    def verify(self, only=None):
        values = self.certificates.values() if only is None else [self.certificates[only]]
        try:
            for path,digest in values:
                if path.is_symlink() or path.stat().st_uid != os.getuid() or stat.S_IMODE(path.stat().st_mode) != 0o400 or hashlib.sha256(path.read_bytes()).hexdigest() != digest:
                    raise RuntimeError('owned-certificate-changed')
        except OSError:
            raise RuntimeError('owned-certificate-changed') from None

    def state(self, label, deadline, maximum=4):
        self.verify()
        helper = Path(__file__).parent / 'swift' / '.build' / 'owned-admin-trust-state'
        args = [str(helper)]
        for path,digest in self.certificates.values(): args.extend([str(path), digest])
        raw = self.run(label, deadline, args, maximum=maximum)
        try:
            if len(raw) > 512: raise ValueError()
            value = json.loads(raw)
            statuses = value['statuses']
            if set(value) != {'statuses','ownedTrustAbsent'} or not isinstance(statuses,list) or len(statuses) != 3 or any(type(code) is not int or not -2**31 < code < 2**31 for code in statuses): raise ValueError()
            absent = all(code in ABSENT for code in statuses)
            if type(value['ownedTrustAbsent']) is not bool or value['ownedTrustAbsent'] != absent: raise ValueError()
        except (ValueError,KeyError,TypeError):
            print('MIXYARD_PHASE ' + label + '-failed', file=sys.stderr, flush=True)
            raise RuntimeError('owned-admin-readback-invalid') from None
        # Only bounded numeric statuses and our fixed phase/absence boolean.
        print('MIXYARD_TRUST_STATE ' + json.dumps({'phase':label,'absent':absent,'statuses':statuses},separators=(',',':')), file=sys.stderr, flush=True)
        if any(code != 0 and code not in ABSENT for code in statuses):
            print('MIXYARD_PHASE ' + label + '-failed', file=sys.stderr, flush=True)
            raise RuntimeError('owned-admin-readback-unavailable')
        return statuses

    def admit(self, deadline):
        if any(code not in ABSENT for code in self.state('admin-before', deadline)):
            print('MIXYARD_PHASE admin-before-failed', file=sys.stderr, flush=True)
            raise RuntimeError('owned-fingerprint-already-trusted')
        self.admitted = True

    def add(self, name, keychain, deadline):
        if not self.admitted or name not in self.certificates:
            raise RuntimeError('owned-admin-not-admitted')
        self.verify(); path,_ = self.certificates[name]
        # Storage remains in the unprivileged owner's private keychain. The
        # elevated command changes only this exact certificate's ADMIN trust.
        self.run('store-' + name, deadline, ['/usr/bin/security','add-certificates','-k',str(keychain),str(path)])
        self.attempted.append(name)  # BEFORE a possibly partial admin add.
        self.run('trust-' + name, deadline, ['sudo','-n','/usr/bin/security','add-trusted-cert','-d','-r','trustRoot','-p','ssl',str(path)])

    def ready(self, deadline):
        if self.state('admin-ready', deadline) != [0,0,0]:
            print('MIXYARD_PHASE admin-ready-failed', file=sys.stderr, flush=True)
            raise RuntimeError('owned-admin-trust-not-present')

    def cleanup(self, deadline):
        if not self.admitted: return True  # Never remove a preexisting record.
        failed = False
        try: before = self.state('admin-precleanup', deadline, maximum=2)
        except RuntimeError: before = None; failed = True
        for name in self.attempted:
            if before is not None and before[self.names.index(name)] in ABSENT: continue
            try:
                self.verify(name); path,_ = self.certificates[name]
                self.run('cleanup-trust-' + name, deadline, ['sudo','-n','/usr/bin/security','remove-trusted-cert','-d',str(path)],maximum=2)
            except RuntimeError: failed = True
        try:
            after = self.state('admin-after', deadline, maximum=2)
            if any(code not in ABSENT for code in after): failed = True
        except RuntimeError: failed = True
        return not failed
