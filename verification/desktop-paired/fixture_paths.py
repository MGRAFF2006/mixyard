"""Private hosted fixture paths only; never change production/store ACL policy."""
import contextlib
import ctypes
import os
from pathlib import Path
import secrets
import shutil
import tempfile


class Windows:
    def __init__(self):
        assert os.name == 'nt' and os.environ.get('GITHUB_ACTIONS') == 'true', 'Owned hosted Windows fixture required'
        self.kernel = ctypes.WinDLL('kernel32', use_last_error=True)
        self.advapi = ctypes.WinDLL('advapi32', use_last_error=True)
        pointer = ctypes.c_void_p
        self.kernel.GetCurrentProcess.restype = pointer
        self.kernel.CloseHandle.argtypes = [pointer]
        self.kernel.LocalFree.argtypes = [pointer]
        self.kernel.LocalFree.restype = pointer
        self.advapi.OpenProcessToken.argtypes = [pointer, ctypes.c_uint32, ctypes.POINTER(pointer)]
        self.advapi.GetTokenInformation.argtypes = [pointer, ctypes.c_int, pointer, ctypes.c_uint32, ctypes.POINTER(ctypes.c_uint32)]
        self.advapi.ConvertSidToStringSidW.argtypes = [pointer, ctypes.POINTER(pointer)]
        self.advapi.ConvertStringSecurityDescriptorToSecurityDescriptorW.argtypes = [ctypes.c_wchar_p, ctypes.c_uint32, ctypes.POINTER(pointer), ctypes.POINTER(ctypes.c_uint32)]
        self.advapi.GetNamedSecurityInfoW.argtypes = [ctypes.c_wchar_p, ctypes.c_int, ctypes.c_uint32, ctypes.POINTER(pointer), pointer, ctypes.POINTER(pointer), pointer, ctypes.POINTER(pointer)]
        self.advapi.GetNamedSecurityInfoW.restype = ctypes.c_uint32
        self.advapi.GetSecurityDescriptorControl.argtypes = [pointer, ctypes.POINTER(ctypes.c_uint16), ctypes.POINTER(ctypes.c_uint32)]
        self.advapi.GetAce.argtypes = [pointer, ctypes.c_uint32, ctypes.POINTER(pointer)]
        self.kernel.CreateDirectoryW.argtypes = [ctypes.c_wchar_p, pointer]
        token = pointer()
        self.check(self.advapi.OpenProcessToken(self.kernel.GetCurrentProcess(), 8, ctypes.byref(token)))
        try:
            size = ctypes.c_uint32()
            self.advapi.GetTokenInformation(token, 1, None, 0, ctypes.byref(size))
            buffer = ctypes.create_string_buffer(size.value)
            self.check(self.advapi.GetTokenInformation(token, 1, buffer, size.value, ctypes.byref(size)))
            self.user = self.sid(ctypes.cast(buffer, ctypes.POINTER(pointer))[0])
        finally: self.kernel.CloseHandle(token)

    @staticmethod
    def check(value):
        if not value: raise OSError(ctypes.get_last_error(), 'Owned fixture Windows API failed')

    def sid(self, value):
        text = ctypes.c_void_p()
        self.check(self.advapi.ConvertSidToStringSidW(value, ctypes.byref(text)))
        try: return ctypes.wstring_at(text.value)
        finally: self.kernel.LocalFree(text)

    def kind(self, sid):
        return 'current-user' if sid == self.user else {
            'S-1-5-18':'system', 'S-1-5-32-544':'administrators', 'S-1-3-4':'owner-rights',
            'S-1-1-0':'everyone', 'S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464':'trusted-installer',
        }.get(sid, 'other')

    def directory(self, path, unsafe=False):
        class Attributes(ctypes.Structure):
            _fields_ = [('length',ctypes.c_uint32),('descriptor',ctypes.c_void_p),('inherit',ctypes.c_int)]
        # Protect at creation, with the actual token USER SID, not OWNER_RIGHTS
        # or a post-creation Set-Acl. unsafe is only the explicit negative fixture.
        descriptor = ctypes.c_void_p()
        text = f'O:{self.user}D:P(A;OICI;FA;;;{self.user})' + ('(A;;FA;;;WD)' if unsafe else '')
        self.check(self.advapi.ConvertStringSecurityDescriptorToSecurityDescriptorW(text, 1, ctypes.byref(descriptor), None))
        try:
            attributes = Attributes(ctypes.sizeof(Attributes), descriptor, 0)
            self.check(self.kernel.CreateDirectoryW(str(path), ctypes.byref(attributes)))
        finally: self.kernel.LocalFree(descriptor)

    def classify(self, path):
        class Header(ctypes.Structure):
            _fields_ = [('revision',ctypes.c_uint8),('unused',ctypes.c_uint8),('size',ctypes.c_uint16),('count',ctypes.c_uint16),('unused2',ctypes.c_uint16)]
        owner = ctypes.c_void_p(); acl = ctypes.c_void_p(); descriptor = ctypes.c_void_p()
        code = self.advapi.GetNamedSecurityInfoW(str(path), 1, 5, ctypes.byref(owner), None, ctypes.byref(acl), None, ctypes.byref(descriptor))
        if code: raise OSError(code, 'Owned fixture ACL readback failed')
        try:
            control = ctypes.c_uint16(); revision = ctypes.c_uint32()
            self.check(self.advapi.GetSecurityDescriptorControl(descriptor, ctypes.byref(control), ctypes.byref(revision)))
            entries = []
            if acl.value:
                count = Header.from_address(acl.value).count
                assert count <= 64, 'Owned fixture ACL exceeds diagnostic bound'
                for index in range(count):
                    ace = ctypes.c_void_p(); self.check(self.advapi.GetAce(acl, index, ctypes.byref(ace)))
                    kind = ctypes.c_uint8.from_address(ace.value).value
                    flags = ctypes.c_uint8.from_address(ace.value+1).value
                    mask = ctypes.c_uint32.from_address(ace.value+4).value
                    trustee = self.kind(self.sid(ace.value+8)) if kind in (0,1) else 'unsupported-ace'
                    mutable = kind == 0 and not flags & 8 and bool(mask & (0x40000000 | 0x10000000 | 0x000D0000 | 0x152))
                    entries.append({'trustee':trustee,'type':kind,'flags':flags,'mask':hex(mask),'mutable':mutable})
            return {'owner':self.kind(self.sid(owner)), 'protected':bool(control.value & 0x1000), 'aces':entries}
        finally: self.kernel.LocalFree(descriptor)


def directory(path):
    if os.name == 'nt': Windows().directory(path)
    else: Path(path).mkdir(mode=0o700)


@contextlib.contextmanager
def temporary(prefix):
    base = Path(tempfile.gettempdir()).resolve(strict=True)
    if os.name != 'nt':
        with tempfile.TemporaryDirectory(prefix=prefix, dir=base) as path: yield Path(path)
        return
    root = base / (prefix + secrets.token_hex(12))
    Windows().directory(root)
    try: yield root
    finally: shutil.rmtree(root)
