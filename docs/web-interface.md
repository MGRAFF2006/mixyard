# Local web console

Implementation contract for the web-first milestone. The Rust `mixyard-web`
binary serves the built `apps/web/dist` interface and JSON API on
`127.0.0.1:8384` by default. It does not start/stop the lifecycle runtime, capture
audio, alter routes, or apply profiles to audio. Non-loopback exposure is outside
this milestone. The existing desktop remains available.

## HTTP contract

JSON success responses contain the result directly. Failures use
`{"error":{"code":"...","message":"..."}}` with a non-2xx status.
Reject unexpected Host/Origin, disallow CORS, bound request sizes and timeouts,
and serve only files underneath the configured static directory. Responses must
not cache API data. The service must not accept a caller-supplied filesystem path.

`GET /api/session` returns
`{"csrfToken":"...","apiVersion":1,"audioAvailable":false}`.
All other endpoints use POST, JSON content type, and the header
`X-Mixyard-Token` containing this process-lifetime token. Same-origin browser
requests are supported; command-line requests without Origin require the token.

| Endpoint | JSON body | Success result |
| --- | --- | --- |
| `/api/status` | `{}` | `{"connected":boolean,"snapshot":object or null,"error":string or null}`; read-only snapshot of the configured runtime instance |
| `/api/validate` | `{"config":Config}` | `{"valid":true}` using core schema-2 validation |
| `/api/inspect` | `{"config":Config,"inventory":Inventory}` | Existing `diagnostics::inspect` result, including failed planning reasons |
| `/api/profiles/read` | `{}` | Existing serialized `profiles::Snapshot` |
| `/api/profiles/edit` | `{"expectedRevision":number,"edit":Edit}` | Updated `profiles::Snapshot`; stale revision is HTTP 409 |

`Edit` is tagged by `kind`: `{"kind":"draft","config":Config}`,
`{"kind":"save","id":string or null,"name":string,"config":Config}`,
`{"kind":"rename","id":string,"name":string}`,
`{"kind":"load","id":string}`, or `{"kind":"undo"}`.
These delegate to the existing profile store, preserving its conflict/atomic
write and recovery behavior. No new profile format or audio activation exists.
The service accepts `--store`, `--instance`, `--port`, and `--assets` options;
defaults use the existing profile default directory, default runtime instance,
port 8384 and `apps/web/dist`. Port 0 is allowed for isolated verification and
the bound address is printed. It must work without an already-running runtime.

## Interface acceptance

The primary workspace is the routing matrix: sources down the left, named mixes
across the top, independent send gain/mute controls in each configured cell.
Output gains/mutes are separate. All controls edit a local draft, with explicit
save, reload, and unsaved-change protection. The browser can import/export schema-2
config and inventory JSON. Example content requires an explicit example action
and stays labelled simulated. Nothing claims live levels or actual sharing.

Named profiles support save, rename, load into the stored draft, and undo through
the API. Fresh reads establish the expected store revision. Conflicts keep local
edits and ask the person to refresh/review, never retry writes automatically.
Runtime disconnected/unavailable, no draft, invalid import, failed inspection,
store conflict, and offline service states have useful recovery actions.
Keyboard controls, narrow screens and visible focus are part of acceptance.

## Design direction

Use a practical audio patch-bay layout with left-aligned text and controls:

```text
Mixyard                 Runtime status        Refresh
[Routing] [Profiles] [Inspect]
Draft actions / saved or unsaved state
Source         Call       Record      Monitor
Microphone     gain/mute  gain/mute   gain/mute
Music          gain/mute  gain/mute   gain/mute
Outputs        independent gain/mute per destination
```

Palette: paper `#f3f6fa`, panel `#ffffff`, ink `#172c43`, cable blue `#2468aa`,
signal teal `#15766e`, caution `#925500`. Use an installed humanist sans family
(`"Segoe UI", "Noto Sans", sans-serif`) for controls and headings; tabular figures
for gains. Keep the matrix as the distinctive functional element, with quiet
navigation and no decorative meters. A generic dashboard of statistic cards
would hide route ownership, so the matrix replaces that initial layout option.
On phones the matrix scrolls within its own region, with persistent source labels.
