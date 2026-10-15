# MicroService configuration

MicroService stores the desired configuration of an OCI service on a MicroVM.
Configurations survive API restarts in the existing SQLite database.
This stage provides configuration CRUD only.
It does not pull images, start replicas, attach domains, or inject secrets.
Deployments, releases, health management, and CrabSecrets are later items in issue #335.

## Fields

| Field | Rule or default |
| --- | --- |
| `name` | Required unique DNS label; 1–63 lowercase letters, digits, or hyphens; start and end with a letter or digit |
| `image` | Required OCI image reference; tag or SHA256 digest; no registry request when saving |
| `runtime` | `vm` only; default `vm` |
| `port` | Required app port; 1–65535 |
| `healthPath` | Absolute HTTP path without query or fragment; default `/health` |
| `domain` | Optional lowercase DNS hostname; no scheme, port, path, or wildcard; default `null` |
| `ram` | Required RAM per replica in MiB; power of two from 128 to 32768 |
| `cpu` | Required vCPU count per replica; 1–32 |
| `replicas` | 1–100; default 1 |
| `startupTimeout` | 1–3600 seconds; default 300 |
| `drainTimeout` | 0–3600 seconds; default 30 |
| `env` | Variable name to plain value; default `{}` |
| `secrets` | Variable name to non-nil CrabSecrets UUID; default `{}`; values are never stored here |

`env` and `secrets` together allow up to 64 entries.
Variable names follow `[A-Za-z_][A-Za-z0-9_]*` and allow up to 256 bytes.
Plain values allow up to 4096 bytes and must not contain NUL bytes.
A variable name cannot appear in both maps.
Secret references are saved without resolving them in this stage.

## REST

| Method | Path | Result |
| --- | --- | --- |
| `GET` | `/api/micro-services` | List configurations in name order |
| `POST` | `/api/micro-services` | Create a configuration; `201` |
| `GET` | `/api/micro-services/{id}` | Read a configuration |
| `PUT` | `/api/micro-services/{id}` | Replace the complete configuration; keep its UUID |
| `DELETE` | `/api/micro-services/{id}` | Delete a configuration; `204` |

POST and PUT use the same JSON body.
Responses add `id` to that body.
Do not include `id` in a request body.
PUT resets omitted optional fields to their defaults.
Use `domain: null`, `env: {}`, and `secrets: {}` to clear those fields.
An invalid request or duplicate name returns `400` without changing the saved configuration.
An unknown UUID returns `404`.
The existing origin rules, request deadline, JSON errors, and 64 KiB body limit apply.

Example `orders.json`:

```json
{
  "name": "orders",
  "image": "ghcr.io/team/orders:1.0",
  "runtime": "vm",
  "port": 8080,
  "healthPath": "/actuator/health",
  "domain": "orders.example.com",
  "ram": 1024,
  "cpu": 2,
  "replicas": 2,
  "startupTimeout": 300,
  "drainTimeout": 30,
  "env": { "SPRING_PROFILES_ACTIVE": "prod" },
  "secrets": {}
}
```

## CLI and dashboard

The commands work on Linux, macOS, and Windows.
They use the normal `--api`, `--host`, and `FIRECRAB_API` endpoint selection.

```sh
firecrab microservice create --file orders.json
firecrab microservice list
firecrab microservice list --json
firecrab microservice get SERVICE_UUID
firecrab microservice update SERVICE_UUID --file orders.json
firecrab microservice delete SERVICE_UUID
```

Create, get, and update print JSON.
Delete prints a JSON receipt.
In the dashboard, open **MicroService** to create, inspect, edit, or delete configurations.
The form supports environment variables and CrabSecrets references.
Saving a configuration does not deploy it.

## Verification

```sh
cargo test -p firecrab-api micro_services
cargo test -p firecrab-cli micro_services
FIRECRAB_E2E_SKIP_GUEST_BOOT=1 npm --prefix firecrab-e2e run test:services
```

API tests cover persistence, full replacement, concurrent name conflicts, and rejected inputs.
CLI tests cover request paths, JSON bodies, secret references, and failures.
Browser tests cover CRUD, validation, Korean labels, and the mobile layout.

## Related

- [API](../reference/api.md)
- [CLI](../reference/cli.md)
- [Dashboard](dashboard.md)
- [OCI images](oci.md)
