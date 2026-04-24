# Deploying vibenet to a bare-metal host

Vibenet runs on one bare-metal (or VM) host fronted by Cloudflare. Public
traffic flows `client -> Cloudflare -> this host:443 (nginx TLS)`. No
intermediate proxy.

The host exposes exactly one public inbound port:

- `:443` - nginx TLS listener, with a Cloudflare Origin CA certificate
  validated by Cloudflare under "Full (strict)" SSL/TLS mode

Plus SSH (`:22`) for operators. Everything else is egress-only.

## Public hostnames

Cloudflare proxies four hostnames directly to this host's `:443`:

| Hostname                   | Served by nginx as       | Backend                |
| -------------------------- | ------------------------ | ---------------------- |
| `vibes.base.org`           | UI + admin + config JSON | static site + grafana  |
| `rpc.vibes.base.org`       | JSON-RPC + WS            | `proxyd` -> base-client |
| `explorer.vibes.base.org`  | vibescan                 | `vibescan`              |
| `faucet.vibes.base.org`    | Faucet UI + drip API     | static site + `vibenet-faucet` |

nginx routes by `Host` header for all four; see
[`etc/vibenet/nginx/vibenet.conf.template`](../nginx/vibenet.conf.template)
and [`etc/vibenet/nginx/vibenet-public.conf.template`](../nginx/vibenet-public.conf.template).

## Prerequisites

1. A linux host (Ubuntu 22.04+ or Debian 12+ recommended) with at least
   8 vCPU / 16 GB RAM / 200 GB disk.
2. Root SSH access.
3. The four DNS A records above pointed at the host's public IP in
   Cloudflare, with orange-cloud (proxy) enabled and SSL/TLS mode set to
   **Full (strict)**.

## 1. Bootstrap the host

As root on the target host:

```bash
curl -fsSL https://raw.githubusercontent.com/base/base/main/etc/vibenet/deploy/bootstrap.sh \
  | sudo VIBENET_REPO_BRANCH=<branch> bash
```

The script is idempotent and does the following:

- Installs Docker, Docker Compose, just, and Foundry
- Creates a `vibenet` unix user with docker group membership
- Clones the repo to `/opt/vibenet/base`
- Enables ufw, allows SSH and `:443/tcp`
- Creates `/etc/vibenet/tls/` (empty; ready for the Origin CA cert)
- Copies `vibenet-env.example` to `vibenet-env` (empty secrets)

## 2. Install the Cloudflare Origin CA certificate

In the Cloudflare dashboard: **SSL/TLS -> Origin Server -> Create
Certificate**. Keep the default key type, request validity of 15 years,
and enter these hostnames:

```
vibes.base.org
*.vibes.base.org
```

Copy the generated PEM bodies to the host:

```bash
sudo tee /etc/vibenet/tls/origin.crt >/dev/null  # paste cert body
sudo tee /etc/vibenet/tls/origin.key >/dev/null  # paste key body
sudo chmod 0644 /etc/vibenet/tls/origin.crt
sudo chmod 0600 /etc/vibenet/tls/origin.key
```

Then set the zone's SSL/TLS encryption mode to **Full (strict)** so
Cloudflare requires this certificate on every connection to the origin.

`just vibe` auto-detects the cert and enables the public `:443` listener.
Hosts without the cert stay local-dev only (loopback ports 18080-18083).

## 3. Fill in secrets

```bash
su - vibenet
cd /opt/vibenet/base
${EDITOR} etc/vibenet/vibenet-env
```

Required values (see [`../vibenet-env.example`](../vibenet-env.example) for
the full list):

- `VIBENET_PUBLIC_BIND_ADDR` - the public IP Cloudflare dials (e.g.
  `64.130.37.133`). Without this the `:443` port only binds to `127.0.0.1`.
- `FAUCET_ADDR` + `FAUCET_PRIVATE_KEY` - generate with `cast wallet new`.
  This address is the only account prefunded in vibenet genesis.
- `ADMIN_HTPASSWD` - bcrypt line from `htpasswd -nbB admin '<password>'`.

## 4. Launch

```bash
just -f etc/docker/Justfile vibe
```

`just vibe` detects `/etc/vibenet/tls/origin.crt` and automatically layers
on `docker-compose.public.yml`, which mounts the TLS cert and publishes
the `:443` listener. The command wipes any existing devnet data, rebuilds
rust images, and brings up:

- L1 anvil + L2 sequencer + consensus + batcher
- `nginx-gateway` (with the public `:443` listener on prod),
  `vibenet-faucet`, `vibenet-setup`, `vibenet-config-renderer`,
  `vibenet-htpasswd`, `proxyd`, `vibescan`
- Jaeger / Prometheus / Grafana (the admin panel)

Give it ~2 minutes. Check progress with `just -f etc/docker/Justfile vibe-logs`.

## 5. Verify

From any internet host:

```bash
curl -s https://vibes.base.org/config.json | jq .title
curl -s -X POST -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":1}' \
  https://rpc.vibes.base.org
curl -s https://explorer.vibes.base.org/ -o /dev/null -w '%{http_code}\n'
curl -s https://faucet.vibes.base.org/status | jq .
```

## Updating to a different branch

Any of the following work; pick whichever fits your operator comfort:

- **SSH + `just vibe`** (simplest; always works):
  ```bash
  ssh vibenet@<host>
  cd /opt/vibenet/base
  git fetch && git checkout <branch or sha> && git pull --ff-only
  just -f etc/docker/Justfile vibe
  ```
- **vibenet-deploy-controller** (optional, from the `vibenet-proxy` repo):
  a Python systemd unit on the host accepts shared-secret-authenticated
  POSTs from an operator's laptop to check out a specific branch/SHA and
  run `just vibe`. See the `host/` folder in that repo.

`just vibe` always wipes chain state, so the new branch starts from fresh
genesis with the faucet prefunded.

## Teardown

```bash
just -f etc/docker/Justfile vibe-down
```

This stops containers and wipes `.devnet/` state. It does not touch
`vibenet-env`, `/etc/vibenet/tls/`, or `contracts.json` history.

## Optional hardening

- **Cloudflare Authenticated Origin Pulls (AOP)**: enable from the zone
  SSL/TLS dashboard, then add `ssl_verify_client on;` plus a trusted CA
  file to `vibenet-public.conf.template`. This makes direct IP access to
  `:443` fail at the TLS handshake (not just at the HTTP-host level).
- **Cloudflare rate limiting rules**: apply IP-based rate limits in the
  dashboard for the four hostnames (the dashboard has preset rules for
  faucets and RPC APIs that are a good starting point). These are
  independent of the nginx `limit_req_zone`s defined in
  `vibenet.conf.template`.
