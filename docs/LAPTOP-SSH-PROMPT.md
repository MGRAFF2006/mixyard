# Prompt to run in an agent thread on the laptop

Establish secure SSH over Tailscale between my laptop and the current homelab-dev
machine, then verify both directions. Do the setup, not just explain it.

Context: laptop is the known Tailscale device laptop (100.66.24.27), Arch Linux,
user humunkulud. Homelab-dev is 100.108.73.48, Debian, user humunkulud. The homelab
answers Tailscale ping to laptop, but SSH to laptop port 22 times out. Laptop
already has documented SSH access to homelab-dev; inspect the actual config.
Do not assume the timeout is an sshd issue: diagnose service, listen address,
firewall and Tailscale grants before changing anything.

Read the machines skill and ~/Documents/config/agent/fleet/README.md plus both
machine records. Identify this session using tailscale status --json. Keep durable
SSH/system config in ~/Documents/config and use its existing install/link flow;
private keys, authorized_keys and known_hosts stay machine-local and out of git.
Don't change git identity or copy credentials/private keys between machines.

1. Verify existing laptop -> homelab-dev access and disk space. Inspect sshd on
   laptop, ssh -G on both sides, listeners, firewall and available Tailscale grants.
   Fix only the actual cause. Preserve existing access. Install openssh only if
   absent and update the laptop package list if needed. Do not expose SSH publicly
   or add router forwards. Keep inbound access limited to the verified Tailscale
   peer/interface; do not casually bind only a boot-time Tailscale IP, which could
   make startup fail. Keep existing firewall management; don't add a second one.
2. Through the existing trusted laptop -> homelab SSH path, create a dedicated
   Ed25519 key on homelab-dev for this connection if absent. Use restrictive file
   permissions. Add only its PUBLIC key to laptop authorized_keys, idempotently,
   restricted with from="100.108.73.48", no-agent-forwarding, no-port-forwarding
   and no-X11-forwarding. Keep shell/SFTP available for development and project
   transfer. Do not overwrite existing keys or print private material.
3. Configure the existing SSH Host laptop entry on homelab-dev through the config
   repo, using the known Tailscale address if hostname DNS is unavailable, user
   humunkulud, this dedicated IdentityFile, IdentitiesOnly yes and normal strict
   host-key verification. Verify laptop's host-key fingerprint over the already
   trusted reverse connection before recording it; never use StrictHostKeyChecking=no.
4. Validate sshd -t and prove key login from homelab-dev before tightening login
   settings. Keep the existing session open during any reload. Disable password
   and keyboard-interactive SSH authentication and root login only after confirmed
   key access and checking Match/include rules. Do not restart unrelated services,
   reboot, or remove recovery access. If elevation is needed, explain the exact
   command through the normal terminal approval flow; don't open decision dialogs.
5. Test from homelab-dev with BatchMode and a short ConnectTimeout: ssh laptop
   confirms identity and a noninteractive shell; verify SFTP/scp using one harmless
   temporary file and clean up only that file. Check the trusted reverse path too.
   If a Tailscale ACL change is needed outside our access, report the exact narrowly
   scoped missing grant; don't weaken the tailnet policy or declare success.
6. On laptop, clone https://github.com/MGRAFF2006/mixyard into ~/Projects/mixyard if
   absent. If it exists, inspect it and preserve dirty work; no resets or overwrites.
   Use independent Git checkouts, not live .git/worktree/auth/cache Syncthing sync.
   The homelab implementation currently has uncommitted work, so a GitHub clone
   contains only published commits. Report that honestly; coordinate any source
   snapshot after its owners stop editing rather than transfer a moving tree.

Update fleet records only with verified connection facts, not secrets. Do not
commit/push. Report actual SSH tests, changed files/services and remaining blockers.
