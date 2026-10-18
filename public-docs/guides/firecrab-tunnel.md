# Firecrab Tunnel

Firecrab Tunnel connects `https://<subdomain>.firecrab.dev` to either an existing
Quick Tunnel or saved Nginx routes. The connection service owns the public
address. Nginx owns path routing and app request distribution.

```text
*.firecrab.dev → connection service → selected Quick Tunnel → VM apps / Nginx
                                  → managed connector → saved Nginx routes → VM apps
```

The service is the standalone `firecrab-tunnel` binary. It stores subdomain
ownership in SQLite and forwards HTTP streams and WebSockets to the current
target. A DNS CNAME alone does not implement this forwarding service.

## Connect an address

1. Open **Pipe → Firecrab Tunnel → Add fixed address**.
2. Enter a subdomain with 3–48 lowercase letters, digits or hyphens.
3. Select **Quick Tunnel** and an existing connector, or select **Nginx routes**
   and a saved route domain.
4. Select **Connect address**.
5. Wait for **Connected**, then open the fixed HTTPS address and its app paths.

Nginx mode publishes every saved path and app pool for the selected hostname.
It creates a dedicated private connector automatically. Quick Tunnel mode
reuses the selected connector. Editing the address can switch between these
modes without changing the public hostname. Unrelated routes and connectors
are retained. Deleting an address removes its remote reservation and only an
exclusively owned connector; a reused user connector remains available.

Ownership credentials are generated automatically and stored in the managed
host's root-only `pipe/firecrab-addresses.json` file. The dashboard and API never
return them. There is no user-facing registration-server URL or token field.
Keep this protected state when migrating a host: losing it loses the ability
to update or remove the existing reservation.

The independent `firecrab-address-updater.service` refreshes target URLs every
30 seconds. It continues when the API restarts. A service lease expires after
180 seconds without refresh. Expired or stopped targets return HTTP 503, while
the subdomain remains reserved. Unreachable service and name conflicts appear
as pending/error states; saving a local entry alone is not public acceptance.

## CLI and API

```sh
# Linux requires root. macOS/Windows forward these commands into the managed guest.
firecrab pipe address connect my-app --quick <connector-uuid>
firecrab pipe address connect my-app --nginx app.internal
firecrab pipe address delete my-app
```

| Interface | Action |
| --- | --- |
| `GET /api/pipe` | Includes non-secret `firecrabTunnels` statuses |
| `POST /api/pipe/firecrab-tunnels` | Save `{subdomain, target}` |
| `DELETE /api/pipe/firecrab-tunnels/{subdomain}` | Remove one owned address |

Target shapes are `{type: "quick_tunnel", id: "<uuid>"}` and
`{type: "nginx", host: "<saved-hostname>"}`. Requests do not accept server URLs,
ownership credentials, executable paths or arbitrary commands.

## Deploy the connection service

This is an operator task for the owner of `firecrab.dev`. Building the client
or saving an address does not deploy the public service or change DNS.

1. Build `cargo build --release -p firecrab-tunnel --locked` and install its
   binary as `/usr/local/bin/firecrab-tunnel` on a persistent Linux host.
2. Create the system user `firecrab-tunnel` with no login shell.
3. Install [firecrab-connection.service](../../packaging/systemd/firecrab-connection.service)
   in `/etc/systemd/system/`, then enable/start it. systemd creates the private
   state directory. Retain its SQLite database and WAL files across upgrades.
4. Create a locally managed Cloudflare Tunnel for this operator host. Adapt
   [the connector example](../../packaging/firecrab-tunnel/cloudflared.yml.example)
   with the actual tunnel UUID and protected credentials file.
5. Add proxied CNAME records for `*` and `tunnel` under `firecrab.dev`, pointing
   to the operator tunnel's `<uuid>.cfargotunnel.com` target. Existing explicit
   records remain separate; inspect conflicts before changing DNS.
6. Run cloudflared as its own persistent service with the adapted config.
   Verify edge TLS covers `tunnel.firecrab.dev` and `*.firecrab.dev`.
7. Check `https://tunnel.firecrab.dev/health` returns 204. Connect an isolated
   canary address through the Firecrab dashboard before allowing normal use.

The operator connector forwards both the control hostname and public wildcard
hostnames to loopback port 8790. The server's `--trust-cloudflare-proxy` option
uses Cloudflare's client-IP header only for local connector connections.
Registration is limited to 16 new names per client IP per hour and 10,000 stored
names. Apply the operator's edge abuse policy before public rollout.

For an operator-approved directly reachable Nginx origin, use
`--nginx-origin https://nginx.example.net`. Only explicitly allowlisted direct
origins are accepted; arbitrary private URLs and metadata endpoints are rejected.
Private management-VM Nginx uses the automatically managed connector instead.

Operator-only `FIRECRAB_TUNNEL_ENDPOINT` can select an alternative HTTPS origin
before the first local registration. The endpoint is persisted with the protected
state. HTTP loopback origins are accepted only for isolated local QA.

Cloudflare supports wildcard ingress rules and wildcard DNS records. Review the
[configuration reference](https://developers.cloudflare.com/tunnel/features/locally-managed-tunnels/configuration-file/)
and [wildcard DNS behavior](https://developers.cloudflare.com/dns/manage-dns-records/reference/wildcard-dns-records/)
when configuring the operator tunnel and existing explicit hostnames.

## Verification

`cargo test -p firecrab-tunnel --locked` covers durable ownership, conflicts,
strict origin validation, real HTTP forwarding, offline leases and WebSockets.
The workspace CI includes these tests. Pipe browser E2E covers both target types,
retargeting, pending/error states, deletion isolation and mobile layout.

On an isolated systemd host, verify both target modes and each saved path. Restart
the selected connector: its temporary URL must change while the fixed hostname
continues reaching the same app after updater reconciliation. Restart only the
API and verify the updater/connector PIDs remain unchanged. Stop the target and
verify 503. Delete one address and verify other addresses, routes and user-owned
connectors remain. Test a second host's conflict without disclosing credentials.

Repeat these checks through actual wildcard DNS/TLS after deployment. Local
fixtures, a running updater or a saved address do not establish public HTTPS.
