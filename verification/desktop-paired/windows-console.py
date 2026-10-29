"""Launch only the owned hosted fixture with explicit inheritable Ctrl-C state.

The runner may ignore Ctrl-C. Windows inherits that flag independently of the
installed handler table, so creating a console alone does not establish the
fixture's signal behavior. No runner or interactive console is signalled here.
"""
import ctypes
import os
import subprocess
import sys

assert os.name == 'nt' and os.environ.get('GITHUB_ACTIONS') == 'true'
kernel = ctypes.WinDLL('kernel32', use_last_error=True)
kernel.SetConsoleCtrlHandler.argtypes = [ctypes.c_void_p, ctypes.c_int]
kernel.SetConsoleCtrlHandler.restype = ctypes.c_int
assert kernel.SetConsoleCtrlHandler(None, False), 'Cannot enable owned child Ctrl-C inheritance'
child = subprocess.Popen(sys.argv[1:], stdin=subprocess.DEVNULL)
# Only the service inherits enabled processing. Keep this console owner alive
# while its child drains; the service's own handler determines a successful exit.
assert kernel.SetConsoleCtrlHandler(None, True), 'Cannot protect owned console launcher'
sys.exit(child.wait())
