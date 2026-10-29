# Production native paired-control qualification

The binary links `apps/desktop/native-control`, exactly as Tauri does. Its only
replacement is an explicitly named vault API double; no fake TLS fetch or
reimplementation of pair/session/request behavior is used. `qualify.py` starts
actual production `mixyard-control` processes with owned private stores and
loopback TLS. Supplemental owned TLS responders exercise malformed/pin/redirect
and raw number boundaries. Evidence contains test labels and binary hashes, never
invitations, private keys, bearer credentials or raw transcripts.

Run through `.github/workflows/desktop-paired.yml` for the three OS SDKs. Local
native dependency installs/builds are deliberately excluded under the current
space budget. Platform vault success and native GUI/physical audio are open gates.
