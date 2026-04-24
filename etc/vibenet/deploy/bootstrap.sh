#!/bin/bash
# Vibenet host bootstrap.
#
# Idempotent script for an Ubuntu/Debian bare-metal host that will run
# vibenet. Installs Docker + just + foundry, creates a `vibenet` unix user,
# clones the repo, and configures ufw to allow SSH + :443 (Cloudflare).
#
# Public traffic flow: client -> Cloudflare -> :443 on this host (nginx TLS
# listener, using a Cloudflare Origin CA cert at /etc/vibenet/tls/). No
# intermediate proxy.
#
# After this runs you still need to:
#   1. Install the Cloudflare Origin CA cert + key at
#      /etc/vibenet/tls/origin.{crt,key}. See etc/vibenet/vibenet-env.example
#      for commands.
#   2. Fill in etc/vibenet/vibenet-env (FAUCET_ADDR, FAUCET_PRIVATE_KEY,
#      ADMIN_HTPASSWD).
#   3. (Optional) Install the vibenet-deploy-controller systemd unit from
#      the vibenet-proxy repo if you want remote GitOps-style branch
#      switches. Not required - `ssh` + `git checkout` + `just vibe` works.
#
# See etc/vibenet/deploy/README.md.
#
# Usage (as root on the target host):
#   curl -fsSL https://raw.githubusercontent.com/base/base/<branch>/etc/vibenet/deploy/bootstrap.sh | sudo bash
#
# Environment:
#   VIBENET_USER           unix user to create (default: vibenet)
#   VIBENET_REPO_URL       git repo to clone (default: https://github.com/base/base.git)
#   VIBENET_REPO_BRANCH    branch to check out (default: main)
#   VIBENET_CHECKOUT_DIR   where to clone (default: /opt/vibenet/base)

set -euo pipefail

VIBENET_USER="${VIBENET_USER:-vibenet}"
VIBENET_REPO_URL="${VIBENET_REPO_URL:-https://github.com/base/base.git}"
VIBENET_REPO_BRANCH="${VIBENET_REPO_BRANCH:-main}"
VIBENET_CHECKOUT_DIR="${VIBENET_CHECKOUT_DIR:-/opt/vibenet/base}"

log() { echo "[bootstrap] $*"; }

if [ "$(id -u)" -ne 0 ]; then
  echo "bootstrap.sh must run as root" >&2
  exit 1
fi

# --- 1. base packages ---------------------------------------------------------
log "installing base packages"
export DEBIAN_FRONTEND=noninteractive
apt-get update -y
apt-get install -y --no-install-recommends \
  ca-certificates curl gnupg git ufw build-essential pkg-config

# --- 2. docker + compose plugin ----------------------------------------------
if ! command -v docker >/dev/null 2>&1; then
  log "installing docker engine"
  install -d -m 0755 /etc/apt/keyrings
  curl -fsSL https://download.docker.com/linux/ubuntu/gpg \
    | gpg --dearmor -o /etc/apt/keyrings/docker.gpg
  chmod a+r /etc/apt/keyrings/docker.gpg
  . /etc/os-release
  echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.gpg] \
    https://download.docker.com/linux/${ID} ${VERSION_CODENAME} stable" \
    > /etc/apt/sources.list.d/docker.list
  apt-get update -y
  apt-get install -y docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin
  systemctl enable --now docker
else
  log "docker already installed"
fi

# --- 3. vibenet unix user -----------------------------------------------------
if ! id -u "${VIBENET_USER}" >/dev/null 2>&1; then
  log "creating user ${VIBENET_USER}"
  useradd --create-home --shell /bin/bash "${VIBENET_USER}"
fi
usermod -aG docker "${VIBENET_USER}"

# --- 4. just ------------------------------------------------------------------
if ! command -v just >/dev/null 2>&1; then
  log "installing just"
  curl --proto '=https' --tlsv1.2 -sSf https://just.systems/install.sh \
    | bash -s -- --to /usr/local/bin
fi

# --- 5. foundry (for cast / forge on the host, optional but handy) -----------
if ! sudo -u "${VIBENET_USER}" bash -lc 'command -v cast >/dev/null 2>&1'; then
  log "installing foundry for ${VIBENET_USER}"
  sudo -u "${VIBENET_USER}" bash -lc 'curl -L https://foundry.paradigm.xyz | bash'
  sudo -u "${VIBENET_USER}" bash -lc '~/.foundry/bin/foundryup -v stable'
fi

# --- 6. clone repo ------------------------------------------------------------
if [ ! -d "${VIBENET_CHECKOUT_DIR}/.git" ]; then
  log "cloning ${VIBENET_REPO_URL} -> ${VIBENET_CHECKOUT_DIR}"
  install -d -o "${VIBENET_USER}" -g "${VIBENET_USER}" "$(dirname "${VIBENET_CHECKOUT_DIR}")"
  sudo -u "${VIBENET_USER}" git clone --branch "${VIBENET_REPO_BRANCH}" \
    "${VIBENET_REPO_URL}" "${VIBENET_CHECKOUT_DIR}"
else
  log "checkout already exists at ${VIBENET_CHECKOUT_DIR}"
fi

# --- 7. firewall: ssh + :443 open to the internet ----------------------------
# Port :443 is the nginx TLS listener that Cloudflare connects to. We leave
# it open to 0.0.0.0/0 because:
#   (a) Cloudflare has 15+ IPv4 ranges that rotate; pinning ufw to them is
#       high-maintenance and breaks whenever Cloudflare grows.
#   (b) The SSL/TLS mode is "Full (strict)" - only Cloudflare will present
#       a cert the Origin CA trusts (our Origin CA cert is itself trusted
#       only by Cloudflare's edge), so stray direct callers get a cert
#       mismatch.
# If we later want belt-and-suspenders, enable Cloudflare Authenticated
# Origin Pulls and add `ssl_verify_client on;` in vibenet-public.conf.
log "configuring ufw"
ufw --force reset
ufw default deny incoming
ufw default allow outgoing
ufw allow OpenSSH
ufw allow 443/tcp comment 'Cloudflare -> nginx'
ufw --force enable

# --- 8. vibenet-env skeleton --------------------------------------------------
ENV_FILE="${VIBENET_CHECKOUT_DIR}/etc/vibenet/vibenet-env"
if [ ! -f "${ENV_FILE}" ]; then
  log "seeding ${ENV_FILE} from example (edit before running just vibe)"
  cp "${VIBENET_CHECKOUT_DIR}/etc/vibenet/vibenet-env.example" "${ENV_FILE}"
  chown "${VIBENET_USER}:${VIBENET_USER}" "${ENV_FILE}"
  chmod 600 "${ENV_FILE}"
fi

# --- 9. tls dir placeholder ---------------------------------------------------
# `just vibe` auto-enables the public overlay when /etc/vibenet/tls/origin.crt
# exists. Create the directory with the right ownership so the operator can
# drop the Cloudflare Origin CA cert + key in without further sudoing.
install -d -o root -g root -m 0755 /etc/vibenet/tls

cat <<EOF

=============================================================================
vibenet host bootstrap complete.

Next steps:

  1. (root) Install the Cloudflare Origin CA certificate:
       Dashboard -> SSL/TLS -> Origin Server -> Create Certificate
       Hostnames: vibes.base.org, *.vibes.base.org
       sudo tee /etc/vibenet/tls/origin.crt >/dev/null   # paste cert
       sudo tee /etc/vibenet/tls/origin.key >/dev/null   # paste key
       sudo chmod 0644 /etc/vibenet/tls/origin.crt
       sudo chmod 0600 /etc/vibenet/tls/origin.key
     Also set the zone's SSL/TLS mode to "Full (strict)" so Cloudflare
     validates this cert.

  2. (${VIBENET_USER}) Fill in vibenet-env and launch the stack:
       su - ${VIBENET_USER}
       cd ${VIBENET_CHECKOUT_DIR}
       \$EDITOR etc/vibenet/vibenet-env   # VIBENET_PUBLIC_BIND_ADDR=<public ip>,
                                          # FAUCET_ADDR, FAUCET_PRIVATE_KEY,
                                          # ADMIN_HTPASSWD, etc.
       just -f etc/docker/Justfile vibe

Inbound ports: SSH (22), 443 (Cloudflare -> nginx TLS).
=============================================================================
EOF
