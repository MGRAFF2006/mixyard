# Read-only observer attachment

`automation::observers` composes the existing [`context::Collector`](context.md)
and [`Controller`](automation-controller.md). It adds a callable production
`NativeProvider`, an injectable `Provider` boundary, explicit saved declarations,
and revocable process-local observation leases. It owns no second controller,
profile store, automatic worker, authentication, routing or audio backend.
The current web activation worker still uses its existing collector/lifecycle;
this PR does not silently change that service's authority or advertise live audio.

`NativeProvider::new(context::Options)` reuses existing Linux providers for exact
mapped executable membership, visible PipeWire endpoint identities, power and
timezone-aware civil time. Their privacy, identity, output, duration and enumeration
limits remain those documented in [context](context.md). There is no application
capture, window/title inspection, audio stream, graph/default/service mutation or
new dependency. Every non-Linux provider retains its existing honest unsupported
state; hosts can implement the same `Provider` trait for independently qualified
platform metadata. Native status is never inferred from a fixture or build.

On Linux, `network.connected` means **at least one non-loopback network interface
reports `operstate=up`**. Virtual interfaces count. It does not prove Internet
access, an IP address, a default route, a paired peer or an SSID. Up to 64 sysfs
entries and 64 bytes per consumed `type`/`operstate` file are read. Loopback is
identified by Linux link type 772, rather than its name. A complete enumeration
with every non-loopback link down/notpresent/lowerlayerdown yields known false;
unreadable, malformed, oversized, unclassified or excessive entries yield unknown
with no boolean, even if another interface reports up. `network.ssid` remains
unsupported. Interface names, addresses and SSIDs are not returned. This is a
bounded synchronous control-worker operation, never an audio callback operation.
The public `linux_link_state(path)` parser allows trusted synthetic filesystem
fixtures; its boolean alone is not platform evidence or authentication.

`apply_declarations` accepts the existing `settings::Declaration` type. Only
`manual.flag` and `device.hardware-monitor-declared`, with exact nonblank entity
subjects, may acquire fresh `UserDeclared` boolean evidence. An explicitly false
flag is known false; an omitted flag is missing/unknown. It validates the entire
capture and application mapping scope before composition, admits the combined
512-fact limit, rejects duplicate/invalid declarations before modification, and
recomputes provider summaries. It does not import native evidence or infer
hardware-monitor capabilities from endpoint direction. Declarations must come
from trusted saved settings, as they do in the existing web activation worker.

## Owner integration contract

Keep one `Attachment<NativeProvider>` in the **existing serialized control owner**.
Do not create another automatic writer. Its clock and provider epoch are the
collector's own monotonic domain. A host integrating remote control must retain
its authenticated target/session checks separately; a lease is not a permission.

1. Call `attachment.refresh(cancelled, &actual_context, &saved_declarations)`.
   This first revokes every previous lease, even when capture/admission later
   fails. The result contains `capture` and an opaque `lease`.
2. Evaluate the graph using that exact capture, its `application_scope`, actual
   owner context and current provider time (`attachment.now_ms()`). Preserve
   `evaluate_scoped` so unmapped applications cannot become known absent.
3. Use `attachment.arm(&mut controller, &lease, &evaluation.token)` for explicit
   arming. Use the same existing controller for observe/settle/manual/hold/cooldown.
   The lease must match the exact epoch, snapshot revision and all four context
   revisions. Evaluation time must be between capture completion and actual now.
4. Freshly capture and reevaluate before each actual commit, obtain the current
   candidate through the controller and pass it through
   `attachment.guard(&mut controller, &lease, &candidate, &current_token)`.
   Retain existing sensitive-path review, profile preview, optimistic store
   revision, write and acknowledgment checks. The observer guard adds no write
   permission and performs no external commit.
5. On disconnection, target switch, revoke or manual ownership change, invalidate
   the attachment immediately and disarm the existing controller under that same
   owner serialization. `reconnect(new_provider)` revokes without restoring a
   lease. Never call raw controller guard/arm to bypass an attachment failure.

`arm` and `guard` disarm the existing controller on failed observation admission,
including expiry and a failed clock read. `revoke` itself only revokes observation
leases: the host must also disarm its controller, so status and timers reflect
the lifecycle event immediately. Check/guard/write must remain serialized with
revoke/reconnect/manual events; the module is not an interthread transaction.

Capture freshness is at most five seconds from completion and from **every known
fact's own observation time/lifetime**, whichever expires first. Missing, future,
incomplete or already expired known facts reject attachment rather than being
relabelled fresh. Unknown and unsupported facts remain available to the evaluator's
explicit unknown branches. Even a still-fresh old evaluation cannot bypass expiry
because each arm/guard checks the provider's actual clock. Any fixture evidence in
the incoming capture prevents arming, including when a saved declaration later
replaces that fact. Fixture evaluation remains useful for simulation.

Each successful capture must advance snapshot revision within its epoch; snapshot
time and owner config/store/inventory/manual revisions cannot regress. Changing
the saved declaration list requires a strictly newer `manual_revision`, even for
true-to-false edits or removal. Reconnection retains these high-water marks. A
provider restart uses a new, never reused epoch; the owner must reset its existing
controller through the established reset protocol. A newly constructed attachment
rejects every earlier object's lease, even if all serialized values match.

No state restores arming after process restart. A host restart must perform fresh
observation, reconciliation, review and explicit arming. `backendAvailable` and
`audioApplied` remain false: neither observer metadata nor a successful guarded
synthetic controller transition proves a live route or physical-device delivery.

## Verification

`cargo test --locked --test automation_observers` covers the actual observer
composition/evaluator/controller boundary using injected synthetic metadata,
including expired/revoked/reconnected leases, stale owner revisions, rejected
acknowledgment after guard revocation, declaration revisions, fixture evidence,
application mapping domains and bounded link-file parsing. The existing context
tests continue to cover device/application/power/civil helper behavior.
Run `cargo clippy --locked --lib --test automation_observers -- -D warnings` and
`cargo fmt --all -- --check` with the ordinary repository checks when resources
permit. These tests do not qualify native Windows/macOS providers, actual network
connectivity, physical audio, or wiring this attachment into the web service.
