# Connect public addresses to Nginx

Firecrab connects an existing public domain or a Cloudflare Quick Tunnel URL to
Nginx. Nginx forwards requests to VM apps by hostname and path and balances
requests across the selected apps.

```text
Public domain ─── DNS / Cloudflare Tunnel ─┐
                                         ├→ Nginx → VM apps
Temporary trycloudflare.com URL ──────────┘
```

A hostname such as `my-app.firecrab.dev` is an example of an existing domain you
control. Firecrab does not allocate names under `firecrab.dev`. This flow uses no
Firecrab registration server, proprietary connection token, reservation API or
address-updater service.

## Use an existing domain

1. Open **Pipe → Routes → Add route**.
2. Enter the full domain, for example `my-app.firecrab.dev`.
3. Enter the path, select the VM app ports, and click **Save**.
4. Add another path under the same hostname if needed. Selecting multiple apps
   balances requests across them.
5. Connect the domain to Nginx:
   - With a separately configured public ingress, point DNS to that ingress and
     configure HTTPS there.
   - For the default private management-VM listener, use the **Cloudflare Tunnel**
     panel. Follow its API-token instructions, load your domains, select the saved
     route domain, and click **Connect and publish**.
6. Open the public hostname with each saved path and check the app response.

Saving a route configures Nginx. It does not by itself create DNS records or make
a loopback listener reachable from the Internet. The Cloudflare setup uses the
real Cloudflare API for domain lookup, tunnel creation and DNS registration.
Existing DNS records are preserved. A real account token and domain permissions
are required for that provider-managed setup.

## Use a Quick Tunnel URL

1. Save the domain/path routes that should be reachable together.
2. Open **Cloudflare Quick Tunnel → Add tunnel**.
3. Enter a name and choose **Nginx routes** as the target.
4. Select the saved route domain and click **Create temporary URL**.
5. Open the issued `https://….trycloudflare.com` URL with the saved paths.

The saved route domain may be a local name such as `pipe.test`. It selects the
Nginx route group; the generated temporary hostname provides public access.
Quick Tunnel needs no user account, API token or personal DNS record. Restarting
a connector changes its URL. The list provides copy, edit, start, stop, restart
and delete actions for each connector.

## CLI and verification

`firecrab pipe status` shows the independent Nginx and connector services. On
macOS and Windows these services run inside the management VM.

A request such as the following checks the Nginx route inside that VM:

```sh
./target/debug/firecrab service shell curl -fsS \
  -H 'Host: my-app.firecrab.dev' http://127.0.0.1:8080/
```

This checks routing, not public DNS or HTTPS. Verify the actual public hostname
separately after its DNS and connection path are configured. See the
[Cloudflare and Quick Tunnel guide](cloudflare-tunnel.md) for provider setup and
the [Pipe guide](pipe.md) for unit ownership and traffic continuity.
