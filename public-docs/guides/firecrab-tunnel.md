# Firecrab Tunnel

Firecrab Tunnel connects a fixed `*.firecrab.dev` address to an existing Quick
Tunnel or saved Nginx routes. Get a connection token from
[the connection dashboard](https://firecrab.dev/dashboard/), then paste it into
the Firecrab app. There is no subdomain input in the Firecrab app.

```text
Connection dashboard → generated address + connection token
Firecrab app → token verification → selected local target
Fixed public address → connection service → Quick Tunnel → Nginx → apps
```

The production dashboard and connection service use registration and per-address
heartbeats. The standalone `firecrab-tunnel` binary in this repository provides
an alternative registry. The client supports both protocols.

## Get a token

1. Open `https://firecrab.dev/dashboard/` and sign in.
2. Select **Issue connection token** and choose its name and expiry.
3. Copy the token and the server-assigned public address. The token is an opaque
   64-character hexadecimal value. The server stores its SHA-256 digest.
4. Keep the token privately. Its original value is displayed only on issuance.

Anyone holding a token can control its assigned address. Use the dashboard's
token controls to revoke or rotate it. Expired and revoked tokens cannot refresh
the connection. The address remains owned by the dashboard account.

## Connect an address

1. Open **Pipe → Firecrab Tunnel → Add fixed address**.
2. Paste the dashboard token into **Connection token**.
3. Select one of the two connection targets:
   - **Quick Tunnel**: select the existing connector directly in **Connection target**.
   - **Nginx routes**: select a saved route domain.
4. Select **Connect address**. The host verifies the token with the connection
   service before changing local routes or saving its ownership secret.
5. Wait for **Connected**, then open the returned public address and app paths.

The **Cloudflare Quick Tunnel** panel creates and manages temporary connectors.
Firecrab Tunnel has no separate Quick Tunnel input; its **Connection target**
list includes each existing connector, including legacy single Quick Tunnels.
If there are no connectors, create one in **Cloudflare Quick Tunnel** or select
Nginx routes.

Nginx mode publishes every saved path and app pool for its hostname. It manages
a dedicated private connector automatically. Quick Tunnel mode reuses the
selected connector. Switching targets keeps the same fixed address and preserves
unrelated routes and connectors. **Edit** uses the secret already saved on the
host; it never reloads the token into the browser.

Deleting a local Firecrab connection removes its exclusively owned connector.
A reused user connector remains. Production dashboard reservations and remote
mappings stay under dashboard control. A mapping to a running reused connector
can still serve requests after local removal; revoke its token in the dashboard
to disable the fixed address. The standalone registry marks removed addresses
offline while retaining dashboard-issued token reservations.

## Ownership and continuity

The managed host stores the endpoint and token in the root-owned 0600
`pipe/firecrab-addresses.json` file. API inspection never returns token values.
Retain this file when migrating a host. Existing installations can continue
using saved legacy ownership tokens for existing reservations; new connections
require a dashboard token. Unknown tokens cannot create new reservations through
the target update API.

The independent `firecrab-address-updater.service` refreshes target URLs every
30 seconds and continues while the API restarts. Production connections send
the current Quick Tunnel URL to the address's heartbeat API. The standalone
registry uses a 180-second lease. A saved local entry or running updater alone
does not prove successful public access.

Token verification or refresh rejects expired and revoked tokens with HTTP 401
or 410. Missing registration endpoints are reported as a service deployment
error, rather than an invalid token.

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
| `POST https://tunnel.firecrab.dev/v1/register` | Bearer token and `{}` resolve `{name, url, heartbeat_url, suspended}` |
| `POST https://{name}.firecrab.dev/__firecrab/heartbeat` | Bearer token and `{tunnel_url}` refresh the current connector URL |

Targets remain `{type: "quick_tunnel", id: "<uuid>"}` and
`{type: "nginx", host: "<saved-hostname>"}`. Supply exactly one of `token` and
`subdomain`; a subdomain can only edit an address already saved on this host.
Server URLs, executable paths and arbitrary commands are not accepted.

## Optional standalone connection service

The standalone Rust registry uses a separate token database. It does not share
accounts or issued tokens with the production dashboard. Its dashboard issues
tokens through `POST /dashboard/tokens` and revokes them through
`POST /dashboard/revoke`. It stores token digests and has no account login.
Issuance permits 16 new addresses per client IP per hour and 10,000 stored addresses.

When registration returns HTTP 404 or 405, the client tries the standalone
registry's `GET /v1/token` lookup and retains its `PUT`/`DELETE /v1/addresses/{name}`
protocol. Older saved registrations retain this protocol by default.

Only deploy this alternative registry on operator-controlled routing. Preserve
the production dashboard's existing routing and token database.

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
