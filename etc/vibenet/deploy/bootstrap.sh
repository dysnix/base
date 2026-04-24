#!/bin/bash
# Vibenet host bootstrap.
#
# Idempotent script for an Ubuntu/Debian bare-metal host that will run vibenet.
# Installs Docker, Just, and Foundry CLI; tightens the firewall to SSH +
# inbound 8443/8445 from the corp-proxy egress CIDR only; and prints next
# steps.
#
# After this runs you still need to:
#   1. Run vibenet-proxy/host/install/bootstrap-origin.sh (origin TLS cert +
#      shared secret at /etc/vibenet/origin/).
#   2. Run vibenet-proxy/host/install/install-deploy-controller.sh (systemd
#      unit on :8445 that accepts branch-switch requests from the corp proxy).
#
# See etc/vibenet/deploy/README.md and etc/vibenet/deploy/DESIGN.md.
#
# Usage (as root on the target host):
#   curl -fsSL https://raw.githubusercontent.com/base/base/<branch>/etc/vibenet/deploy/bootstrap.sh | sudo bash
#
# Environment:
#   VIBENET_USER           unix user to create (default: vibenet)
#   VIBENET_REPO_URL       git repo to clone (default: https://github.com/base/base.git)
#   VIBENET_REPO_BRANCH    branch to check out (default: main)
#   VIBENET_CHECKOUT_DIR   where to clone (default: /opt/vibenet/base)
#   VIBENET_CORP_CIDR      corp-proxy egress CIDR allowed to reach 8443/8445
#                          (default: unset; skips those ufw rules)

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

# --- 7. firewall: ssh + origin/deploy ports restricted to corp proxy ---------
log "configuring ufw"
ufw --force reset
ufw default deny incoming
ufw default allow outgoing
ufw allow OpenSSH
if [ -n "${VIBENET_CORP_CIDR:-}" ]; then
  log "allowing tcp 8443 (origin) and 8445 (deploy) from ${VIBENET_CORP_CIDR}"
  ufw allow from "${VIBENET_CORP_CIDR}" to any port 8443 proto tcp
  ufw allow from "${VIBENET_CORP_CIDR}" to any port 8445 proto tcp
else
  log "WARNING: VIBENET_CORP_CIDR unset; 8443/8445 not opened."
  log "         Add ufw allow rules manually before wiring up the corp proxy."
fi
ufw --force enable

# --- 8. vibenet-env skeleton --------------------------------------------------
ENV_FILE="${VIBENET_CHECKOUT_DIR}/etc/vibenet/vibenet-env"
if [ ! -f "${ENV_FILE}" ]; then
  log "seeding ${ENV_FILE} from example (edit before running just vibe)"
  cp "${VIBENET_CHECKOUT_DIR}/etc/vibenet/vibenet-env.example" "${ENV_FILE}"
  chown "${VIBENET_USER}:${VIBENET_USER}" "${ENV_FILE}"
  chmod 600 "${ENV_FILE}"
fi

cat <<EOF

=============================================================================
vibenet host bootstrap complete.

Next steps:

  1. (root) Bootstrap origin identity from the vibenet-proxy repo:
       sudo HOST_IP=<public ip> bash vibenet-proxy/host/install/bootstrap-origin.sh
     This writes the self-signed TLS cert + shared secret to /etc/vibenet/origin/.

  2. (root) Install the deploy controller:
       sudo bash vibenet-proxy/host/install/install-deploy-controller.sh

  3. (${VIBENET_USER}) Fill in vibenet-env and launch the stack:
       su - ${VIBENET_USER}
       cd ${VIBENET_CHECKOUT_DIR}
       \$EDITOR etc/vibenet/vibenet-env   # VIBENET_ORIGIN_BIND_ADDR, FAUCET_ADDR,
                                          # FAUCET_PRIVATE_KEY, ADMIN_HTPASSWD, etc.
       just -f etc/docker/Justfile vibe

Inbound ports: SSH (22), plus 8443 (nginx TLS origin) and 8445 (deploy
controller) restricted to VIBENET_CORP_CIDR if it was set.
=============================================================================
EOF
