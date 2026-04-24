# Deploying vibenet to a bare-metal host

Vibenet runs on one bare-metal (or VM) host sitting behind a corporate proxy
on approved hardware. Public traffic flows
`client -> CDN -> corp proxy -> this host:8443 (nginx TLS origin)`.
See [`DESIGN.md`](./DESIGN.md) for the end-to-end architecture.

The host exposes exactly two inbound ports to the corp proxy:

- `:8443` — nginx TLS origin listener, guarded by a pinned self-signed cert
  and the `X-Vibenet-Origin-Auth` shared-secret header
- `:8445` — `vibenet-deploy-controller` (from the `vibenet-proxy` repo), which
  accepts branch-switch requests using the same shared secret

Everything else is egress-only.

## Prerequisites

1. A linux host (Ubuntu 22.04+ or Debian 12+ recommended) with at least
   8 vCPU / 16 GB RAM / 200 GB disk.
2. Root SSH access.
3. The corporate proxy service from the `vibenet-proxy` repo deployed on
   approved hardware, with its egress CIDR noted for the host firewall.

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
- Enables ufw, allows SSH and restricts 8443/8445 to the corp-proxy CIDR
- Copies `vibenet-env.example` to `vibenet-env` (empty secrets)

## 2. Bootstrap the origin identity (one-time)

Origin identity (self-signed TLS cert + `VIBENET_ORIGIN_AUTH_SECRET`) is
owned by the [`vibenet-proxy`](https://github.com/coinbase-infra/vibenet-proxy)
repo and lives at `/etc/vibenet/origin/`, outside the `base` checkout so it
survives `git checkout` and `just vibe down -v`.

```bash
sudo HOST_IP=<public ip> ORIGIN_HOSTNAME=vibenet-origin.internal \
  bash vibenet-proxy/host/install/bootstrap-origin.sh
```

This writes:

- `/etc/vibenet/origin/tls/vibenet-origin.{crt,key}` — 10y ECDSA P-256 cert
- `/etc/vibenet/origin/origin.env` — `VIBENET_ORIGIN_AUTH_SECRET=<hex>`

Push the cert and the secret into the corp config service so the proxy can
pin them.

## 3. Install the deploy controller

Also from the `vibenet-proxy` repo, install the systemd unit that listens on
`:8445` and executes `git fetch && git checkout <branch> && just vibe` when
the corp proxy posts a branch-switch request:

```bash
sudo bash vibenet-proxy/host/install/install-deploy-controller.sh
```

After this the `vibenet-deploy-controller.service` unit is active and
listening. Branch switches no longer require SSH.

## 4. Fill in secrets

```bash
su - vibenet
cd /opt/vibenet/base
${EDITOR} etc/vibenet/vibenet-env
```

Required values (see `etc/vibenet/vibenet-env.example` for details):

- `VIBENET_ORIGIN_BIND_ADDR` — the public IP the corp proxy dials (e.g.
  `64.130.37.133`). Without this the 8443 port only binds to `127.0.0.1`.
- `FAUCET_ADDR` + `FAUCET_PRIVATE_KEY` — generate with `cast wallet new`.
  This address is the only account prefunded in vibenet genesis.
- `ADMIN_HTPASSWD` — bcrypt line from `htpasswd -nbB admin '<password>'`.

## 5. Launch

```bash
just -f etc/docker/Justfile vibe
```

`just vibe` detects `/etc/vibenet/origin/origin.env` and automatically layers
on `docker-compose.origin.yml`, which mounts the TLS cert, loads the shared
secret into the nginx-gateway env, and publishes the 8443 origin listener.

The command wipes any existing devnet data, rebuilds rust images, and brings
up:

- L1 anvil + L2 sequencer + consensus + batcher
- `nginx-gateway` (with the origin TLS listener on prod), `vibenet-faucet`,
  `vibenet-setup`, `vibenet-config-renderer`, `vibenet-htpasswd`, `proxyd`
- Jaeger / Prometheus / Grafana (the admin panel)

Give it ~2 minutes. Check progress with `just -f etc/docker/Justfile vibe-logs`.

## 6. Verify

From the corp proxy (or any host allowlisted for 8443):

```bash
# Should 403 without the header
curl -sk -o /dev/null -w '%{http_code}\n' \
  --resolve vibenet.base.org:8443:<host ip> \
  https://vibenet.base.org:8443/

# Should 200 with the shared secret
curl -sk -o /dev/null -w '%{http_code}\n' \
  -H "X-Vibenet-Origin-Auth: $(sudo cat /etc/vibenet/origin/origin.env | cut -d= -f2)" \
  --resolve vibenet.base.org:8443:<host ip> \
  https://vibenet.base.org:8443/
```

## Updating to a different branch

Preferred path (no SSH): trigger the deploy controller from the corp proxy,
which is already authenticated with the shared secret.

Legacy path (SSH, still works):

```bash
cd /opt/vibenet/base
git fetch && git checkout <branch> && git pull
just -f etc/docker/Justfile vibe
```

`just vibe` always wipes chain state, so the new branch starts from fresh
genesis with the faucet prefunded.

## Teardown

```bash
just -f etc/docker/Justfile vibe-down
```

This stops containers and wipes `.devnet/` state. It does not touch
`vibenet-env`, `/etc/vibenet/origin/`, or `contracts.json` history.
