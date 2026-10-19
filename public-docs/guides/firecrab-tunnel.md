# Firecrab Tunnel

Firecrab Tunnel connects a fixed `*.firecrab.dev` address to an existing Quick
Tunnel or saved Nginx routes. Get a connection token from
[the connection dashboard](https://firecrab.dev/dashboard/), then paste it into
the Firecrab app. There is no subdomain input in the app or token dashboard.

```text
Connection dashboard → generated address + connection token
Firecrab app → token verification → selected local target
Fixed public address → connection service → Quick Tunnel → Nginx → apps
```

The standalone `firecrab-tunnel` binary serves the token dashboard and forwards
HTTP streams and WebSockets. DNS records alone do not implement this service.
The operator must deploy it and route both the dashboard and public hostnames.

## Get a token

1. Open `https://firecrab.dev/dashboard/`.
2. Select **Create connection token**.
3. Copy the token and the server-assigned public address. The token is an opaque
   64-character hexadecimal value. The server stores its SHA-256 digest.
4. Keep the token privately. It is not saved in browser storage or recoverable
   from the server. Refreshing or leaving the page clears the displayed token.

Anyone holding a token can control its assigned address. To revoke it, paste it
into **Revoke a token** on the same dashboard. Revocation disables its forwarding
and permanently rejects future token verification and target refreshes. The
address stays reserved. A new token receives a new address.

Issuance permits 16 new addresses per client IP per hour and 10,000 stored
addresses. There is no login, account recovery or address transfer UI. Operators
must set an appropriate edge abuse policy before public rollout.

## Connect an address

1. Open **Pipe → Firecrab Tunnel → Add fixed address**.
2. Paste the dashboard token into **Connection token**.
3. Select one of the two connection targets:
   - **Quick Tunnel**: select an existing connector.
   - **Nginx routes**: select a saved route domain.
4. Select **Connect address**. The host verifies the token with the connection
   service before changing local routes or saving its ownership secret.
5. Wait for **Connected**, then open the returned public address and app paths.

The separate Quick Tunnel panel is removed. Existing connectors, management APIs
and saved targets remain available. The Firecrab Tunnel selector includes legacy
single Quick Tunnels. If there are no existing connectors, select Nginx routes
or provision a connector using the [Quick Tunnel API](cloudflare-tunnel.md#api).

Nginx mode publishes every saved path and app pool for its hostname. It manages
a dedicated private connector automatically. Quick Tunnel mode reuses the
selected connector. Switching targets keeps the same fixed address and preserves
unrelated routes and connectors. **Edit** uses the secret already saved on the
host; it never reloads the token into the browser.

Deleting a local Firecrab connection removes its exclusively owned connector.
A reused user connector remains. Dashboard-issued addresses become offline but
stay reserved to their token, so that token can reconnect later. Use dashboard
revocation to permanently invalidate the token.

## Ownership and continuity

The managed host stores the endpoint and token in the root-owned 0600
`pipe/firecrab-addresses.json` file. API inspection never returns token values.
Retain this file when migrating a host. Existing installations can continue
using saved legacy ownership tokens for existing reservations; new connections
require a dashboard token. Unknown tokens cannot create new reservations through
the target update API.

The independent `firecrab-address-updater.service` refreshes target URLs every
30 seconds and continues while the API restarts. Its lease expires after 180
seconds without refresh. Expired or stopped targets return HTTP 503. A saved
local entry or running updater alone does not prove successful public access.

Revoked tokens return HTTP 410. The client reports that status and can remove
the local connection without trying to recreate the revoked reservation.

## CLI and API

Read the token from a private file so it never appears in process arguments.
Linux requires root. macOS/Windows forward the intent into the managed guest.

```sh
firecrab pipe address connect --token-file /private/token.txt --quick <connector-uuid>
firecrab pipe address connect --token-file /private/token.txt --nginx app.internal
# Edit an existing local connection without resubmitting its saved token.
firecrab pipe address connect <saved-address-label> --nginx app.internal
firecrab pipe address delete <saved-address-label>
```

| Interface | Action |
| --- | --- |
| `GET /api/pipe` | Includes non-secret `firecrabTunnels` statuses |
| `POST /api/pipe/firecrab-tunnels` | Connect with `{token, target}` |
| `POST /api/pipe/firecrab-tunnels` | Edit a saved connection with `{subdomain, target}` |
| `DELETE /api/pipe/firecrab-tunnels/{subdomain}` | Disconnect one saved address |
| `POST /dashboard/tokens` | Same-origin JSON `{}` issues a token and public URL |
| `POST /dashboard/revoke` | Same-origin JSON `{token}` permanently revokes it |
| `GET /v1/token` | Bearer token resolves `{subdomain, publicUrl}` on the control hostname |

Targets remain `{type: "quick_tunnel", id: "<uuid>"}` and
`{type: "nginx", host: "<saved-hostname>"}`. Supply exactly one of `token` and
`subdomain`; a subdomain can only edit an address already saved on this host.
Server URLs, executable paths and arbitrary commands are not accepted.

## Deploy the connection service

1. Build `cargo build --release -p firecrab-tunnel --locked` and install the binary
   as `/usr/local/bin/firecrab-tunnel` on a persistent Linux host.
2. Create the `firecrab-tunnel` system user with no login shell.
3. Install [firecrab-connection.service](../../packaging/systemd/firecrab-connection.service)
   and enable it. Retain its SQLite database and WAL files across upgrades.
4. Adapt [the connector example](../../packaging/firecrab-tunnel/cloudflared.yml.example)
   with the operator tunnel UUID and protected credentials file.
5. Route `firecrab.dev/dashboard/` and its assets/mutation paths to the service.
   Preserve the existing apex website's other paths. If that website uses another
   ingress, configure a path proxy there rather than replacing its DNS blindly.
6. Route `tunnel.firecrab.dev` and `*.firecrab.dev` to the operator connector.
   Verify wildcard DNS, edge TLS, and `https://tunnel.firecrab.dev/health` (204).
7. Issue a canary token and verify both targets through their actual public URL.

The operator connector forwards to loopback port 8790. `--trust-cloudflare-proxy`
uses the client-IP header only for connections from a local connector. Dashboard
mutations require same-origin JSON; token responses are not cacheable. The token
table is created alongside existing address records on startup.

For operator-approved directly reachable Nginx origins, use
`--nginx-origin https://nginx.example.net`. Only allowlisted direct origins are
accepted. Private management-VM routes use their automatically managed connector.
Operator-only `FIRECRAB_TUNNEL_ENDPOINT` selects an alternative HTTPS origin
before the first local registration. HTTP loopback is supported for isolated QA.

## Manual verification

Verify dashboard issuance, masking/copy, refresh behavior and revocation. Connect
both targets, switch between them, restart the API, stop/start the source and
delete one connection while retaining a peer. Verify token omission in API
inspection and logs. Verify that revocation survives a service restart and the
updater cannot revive it. Repeat through actual wildcard DNS/TLS after deployment.

Regression cases describe issuance quotas, origin checks, secret redaction,
ownership, revocation, reconnects and forwarding. Compile checks alone do not
establish working public HTTPS; verify the operator deployment separately.
