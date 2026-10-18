# Publish apps and Nginx routes with Quick Tunnels

Cloudflare Quick Tunnel creates temporary HTTPS addresses such as
`https://random-name.trycloudflare.com`. A Cloudflare account, API token, and
personal domain are not required. The address changes when its connector is
started again. See [Cloudflare Quick Tunnels](https://developers.cloudflare.com/tunnel/get-started/quick-tunnels/).

Firecrab manages a list of tunnels. Each entry has a name, a target, its own
connector service, and its own current URL.

## Requirements

- A running Firecrab Linux host or managed guest with Nginx installed.
- For VM app targets, running VMs with IPv4 addresses and registered TCP ports.
- For Nginx route targets, a saved domain/path route group.
- Outbound access for Cloudflare Tunnel. Firecrab installs its pinned,
  checksum-verified cloudflared connector if it is missing.

On macOS and Windows, the connector runs inside the managed Linux guest.

## Create a tunnel

1. Open **Pipe** and find **Cloudflare Quick Tunnel**.
2. Select **Add tunnel**.
3. Enter a **Name** that identifies this public entry.
4. Choose a **Target**:
   - **VM apps**: select one or more registered VM app ports. Requests are
     distributed between the selected apps.
   - **Nginx routes**: select a saved **Route domain**. Firecrab publishes every
     saved path in that domain group. The form previews those paths.
5. Select **Create temporary URL**.
6. Wait for the HTTPS link in the list. The first response can arrive before the
   connector has obtained its URL.
7. Repeat **Add tunnel** to publish another app or route group independently.

Existing domain/path routes remain unchanged. Creating a tunnel does not deploy
an app or register a personal DNS record.

A route domain such as `pipe.test` is a saved routing selector. It does not have
to be registered in public DNS. The generated temporary hostname provides public
access to that selected domain's paths.

## Use a route group's paths

The list shows the selected domain and its paths. The public link and the copy
icon beside it contain the base address. Append the required path when accessing a routed app.

| Saved paths in the selected domain | Public request | Result |
| --- | --- | --- |
| `/` and `/api` | `https://…trycloudflare.com/` | Root route's app pool |
| `/` and `/api` | `https://…trycloudflare.com/api` | API route's app pool |
| `/api` only | `https://…trycloudflare.com/api` | API route's app pool |
| `/api` only | `https://…trycloudflare.com/` | HTTP 404 |
| `/api` only | `https://…trycloudflare.com/apiculture` | HTTP 404 |

Nginx keeps the saved path boundaries and longest-path selection. It does not
strip the path. Updating a saved route updates its tunnel destinations through
Nginx reload while keeping the current connector URL.

## Edit, copy, and delete

Open **⋯** in the selected tunnel row:

- **Edit** opens its name and target. Select **Save** once to apply changes, or
  **Cancel** to discard its form inputs. Editing an entry keeps other tunnels.
- **Delete** stops and removes that tunnel's local service and configuration.
  It leaves other tunnels, app VMs, and saved Nginx routes in place.

Use the copy icon beside an address to copy it. A check mark confirms success.
A tunnel's save or delete operation does not save unfinished route or Nginx
listen-address changes. Those drafts remain until saved in their own section.

A route-target tunnel requires its selected domain to retain at least one saved
route. Change or delete the tunnel before removing that domain's last route.

An existing single Quick Tunnel appears in the list as **Quick Tunnel**. Its
first edit/save migrates it to an individual service and can change its URL.
Existing named Cloudflare tunnel manifests remain compatible with the CLI.

## Control one tunnel

Each row's **⋯** menu also provides **Start**, **Stop**, and **Restart**.

- **Stop** ends that tunnel's public access. Its row and target remain saved.
- **Start** runs its saved target and obtains a new temporary address.
- **Restart** replaces only that connector and obtains a new temporary address.

Other tunnels keep their current running or stopped state. Editing or deleting
another entry does not start a stopped connector. Stopping a connector keeps Nginx available for local
requests. API process restarts leave Nginx and the connectors under systemd
ownership. A connector restart can interrupt its existing requests.

Refreshing the dashboard restores the list and each running connector's current
URL. Inactive or failed entries hide their old link. The URL is runtime state,
rather than a permanent saved address.

## Troubleshooting

| Result | Action |
| --- | --- |
| Create is unavailable | Enter a name and select running app ports or a saved route domain. |
| No VM choices | Check VM state, IPv4, TCP ports, and API availability. |
| No route domains | Save a Nginx domain/path route before selecting this target mode. |
| Base link returns 404 | Append a path listed for that route domain, such as `/api`. |
| Preparing URL | Check that row's service state and logs. Wait for the next poll. |
| Old link fails after restart | Copy the new URL from that tunnel's row. |
| Removing a domain is rejected | Change or delete tunnels targeting that domain first. |

## API

| Endpoint | Request | Result |
| --- | --- | --- |
| `POST /api/pipe/quick-tunnels` | `{id: null, name, target}` | Create an entry with a generated UUID |
| `POST /api/pipe/quick-tunnels` | `{id, name, target}` | Update that entry |
| `DELETE /api/pipe/quick-tunnels/{id}` | None | Delete that entry |
| `POST /api/pipe/quick-tunnels/{id}/service` | `{action: "start", "stop" or "restart"}` | Control that connector |

An app target is `{type: "apps", upstreams: ["VM_IP:PORT"]}`. A route target is
`{type: "routes", host: "pipe.test"}`. These operations use the server's saved
configuration and retain unrelated tunnels and routes.

`GET /api/pipe` includes `quickTunnels` entries with their name, target, unit
state, and current `publicUrl`. A URL can be null while it is being issued.
The saved manifest stores definitions in `config.quickTunnels`. The legacy
`quickTunnel` field remains compatible.

The endpoints use the existing authentication, origin controls, and fixed helper
operations. Automated browser tests use API fixtures. Real external HTTPS
reachability requires a separate check against each generated URL.

## Use a stable Cloudflare domain

The separate **Cloudflare Tunnel** panel publishes saved Nginx routes using a
Cloudflare domain you own. Its address remains the same when its connector
restarts. One regular connector can run alongside the Quick Tunnel list.
If a pre-list Quick Tunnel is still configured, open its **Edit** form and select
**Save** once first. This migration changes its temporary URL once and enables
regular tunnel publishing. API token input and domain lookup are available before
this migration.

1. Register your domain in Cloudflare and complete nameserver activation.
2. Save an Nginx route for an unused hostname in that domain, such as
   `app.example.com`. Add the required path routes and VM apps.
3. Find the API token input in **Cloudflare Tunnel**. It is visible whenever no
   regular tunnel is connected, including while a pre-list Quick Tunnel is running.
4. Open **How to create an API token**. Create a scoped token with Account /
   Cloudflare Tunnel / Edit, Zone / DNS / Edit, and Zone / Zone / Read.
5. Paste the token and select **Load domains**.
6. Select the Cloudflare domain and the saved **Route domain**.
7. Select **Connect and publish**. Firecrab creates a tunnel and DNS record,
   installs its verified connector if necessary, and checks its connection.
8. Wait for DNS to become available, then open the fixed HTTPS address. Use the
   adjacent copy icon to copy it. Existing DNS records are never overwritten.

The API token stays in the setup request and is cleared from the form after
success or **Clear input**. Only the connector credentials remain protected on
the Linux host. See [Cloudflare setup](https://developers.cloudflare.com/tunnel/get-started/).

Use the Cloudflare Tunnel service row's **Start**, **Stop**, and **Restart**
actions to control the regular connector. **Disconnect** removes its local unit
and credentials while preserving Nginx routes and other Quick Tunnels. Remote
Cloudflare DNS and tunnel records remain in the account for separate management.

`POST /api/pipe/cloudflare/zones` lists available domains using `{apiToken}`.
`POST /api/pipe/cloudflare/connect` accepts `{apiToken, zoneId, hostname, config}`.
The helper retains the latest saved listener, unrelated routes, and Quick Tunnels.
`POST /api/pipe/cloudflare/disconnect` removes only the local regular connector.
The setup endpoints retain authentication and origin checks while allowing their
bounded installation and provider operations to complete.

Real account and owned-domain acceptance checks require a user-provided token.
Automated tests use provider fixtures and do not create resources in user accounts.
