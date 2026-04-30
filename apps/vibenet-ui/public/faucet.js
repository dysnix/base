// base vibenet faucet page.
//
// Served from faucet.vibes.base.org in prod (its own Cloudflare-fronted
// hostname) and from http://localhost:18083 for local dev. Calls the
// faucet service at same-origin /status, /drip, and /drip-usdv. The drip
// form has two submit buttons ("Drip ETH" / "Mint USDV"); the clicked
// button's `value` picks which asset to drip. USDV drips go to a separate
// endpoint that reads the token address from the shared contracts.json
// and calls mint() on it.

// Mirror the explorer-URL logic from app.js so the USDV address in the
// status line can link directly to its explorer page. Kept inline (rather
// than imported) because these pages ship as plain <script> tags with no
// bundler.
function isLocalHost(host) {
  return host === "localhost" || host === "127.0.0.1" || host === "0.0.0.0";
}

function buildExplorerUrl() {
  const host = location.hostname;
  if (isLocalHost(host)) {
    // Local dev publishes the faucet on the UI base port + 3 and the
    // explorer on +2. `location.port` is +3 here, so the explorer is -1.
    const faucetPort = parseInt(location.port || "80", 10);
    const explorerPort = faucetPort - 1;
    return `${location.protocol}//${host}:${explorerPort}`;
  }
  return "https://explorer.vibes.base.org";
}

function buildUiUrl() {
  const host = location.hostname;
  if (isLocalHost(host)) {
    // UI is at faucetPort - 3.
    const faucetPort = parseInt(location.port || "80", 10);
    const uiPort = faucetPort - 3;
    return `${location.protocol}//${host}:${uiPort}`;
  }
  return "https://vibes.base.org";
}

function formatEth(wei) {
  return (Number(wei || 0) / 1e18).toFixed(4);
}

function formatUsdv(units) {
  // USDV has 6 decimals. Default faucet drip is a whole number of dollars
  // so two-decimal display is plenty.
  const n = Number(units || 0) / 1e6;
  return n.toLocaleString(undefined, { maximumFractionDigits: 2 });
}

function shortAddress(value) {
  if (!value || value.length <= 14) return value || "";
  return `${value.slice(0, 6)}…${value.slice(-4)}`;
}

function explorerLink(path, label, className) {
  const a = document.createElement("a");
  a.href = `${buildExplorerUrl()}${path}`;
  a.target = "_blank";
  a.rel = "noopener";
  if (className) a.className = className;
  a.textContent = label;
  return a;
}

function statusPill(label, value) {
  const pill = document.createElement("div");
  pill.className = "faucet-pill";
  const k = document.createElement("span");
  k.className = "faucet-pill-key";
  k.textContent = label;
  const v = document.createElement("span");
  v.className = "faucet-pill-value";
  v.textContent = value;
  pill.append(k, v);
  return pill;
}

async function loadStatus() {
  const el = document.getElementById("faucet-status");
  try {
    const res = await fetch("/status", { cache: "no-store" });
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const s = await res.json();
    const eth = formatEth(s.balance_wei);
    el.innerHTML = "";

    const summary = document.createElement("div");
    summary.className = "faucet-summary";
    summary.append(statusPill("ETH", `${eth} ETH`));
    summary.append(statusPill("USDV", s.usdv_address ? "Ready to print" : "Not deployed"));
    el.appendChild(summary);

    const footerLinks = document.getElementById("faucet-footer-links");
    if (footerLinks) {
      footerLinks.innerHTML = "";
      footerLinks.append(
        explorerLink(
          `/address/${s.address}`,
          `Faucet ${shortAddress(s.address)}`,
          "address-chip",
        ),
      );
    }
    if (s.usdv_address) {
      footerLinks?.append(
        explorerLink(
          `/address/${s.usdv_address}`,
          `USDV ${shortAddress(s.usdv_address)}`,
          "address-chip",
        ),
      );
    }

    const ethButton = form.querySelector('button[value="eth"]');
    if (ethButton) ethButton.textContent = `Request ${formatEth(s.drip_wei)} ETH`;
    const usdvButton = form.querySelector('button[value="usdv"]');
    if (usdvButton && s.usdv_drip_units) {
      usdvButton.textContent = `Request ${formatUsdv(s.usdv_drip_units)} USDV`;
      usdvButton.disabled = !s.usdv_address;
    }
  } catch (err) {
    el.textContent = `Could not load faucet status: ${err.message}`;
  }
}

function setResultPending(token) {
  const resultEl = document.getElementById("drip-result");
  resultEl.className = "drip-result pending";
  resultEl.textContent = token === "usdv" ? "Minting USDV..." : "Requesting ETH...";
}

function setResultSuccess(token, body) {
  const resultEl = document.getElementById("drip-result");
  const asset = token === "usdv" ? "USDV" : "ETH";
  resultEl.className = "drip-result success";
  resultEl.innerHTML = "";

  const title = document.createElement("div");
  title.className = "drip-result-title";
  title.textContent = `${asset} request submitted`;

  const meta = document.createElement("div");
  meta.className = "drip-result-meta";
  meta.append("Transaction ");
  meta.append(explorerLink(`/tx/${body.tx_hash}`, shortAddress(body.tx_hash), "tx-link"));
  meta.append(" -> ");
  meta.append(explorerLink(`/address/${body.to}`, shortAddress(body.to), "tx-link"));
  if (token === "usdv" && body.token) {
    meta.append(" via ");
    meta.append(explorerLink(`/address/${body.token}`, "USDV", "tx-link"));
  }

  resultEl.append(title, meta);
}

function setResultError(token, message) {
  const resultEl = document.getElementById("drip-result");
  resultEl.className = "drip-result error";
  resultEl.textContent = `${token === "usdv" ? "USDV request" : "ETH request"} failed: ${message}`;
}

const form = document.getElementById("drip-form");
form.addEventListener("submit", async (ev) => {
  ev.preventDefault();
  const addr = document.getElementById("addr").value.trim();
  // submitter is set when a named submit button is clicked; default to ETH.
  const token = (ev.submitter && ev.submitter.value) || "eth";
  const buttons = form.querySelectorAll("button");
  buttons.forEach((b) => (b.disabled = true));
  setResultPending(token);
  try {
    const endpoint = token === "usdv" ? "/drip-usdv" : "/drip";
    const res = await fetch(endpoint, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ address: addr }),
    });
    const body = await res.json().catch(() => ({}));
    if (!res.ok) {
      // nginx-level 429s return an HTML body (not JSON), so `body.error`
      // is empty - fall back to a friendly message for that case.
      const reason =
        body.error ||
        (res.status === 429
          ? "rate limited - wait a minute and try again"
          : `HTTP ${res.status}`);
      throw new Error(reason);
    }
    setResultSuccess(token, body);
  } catch (err) {
    setResultError(token, err.message);
  } finally {
    buttons.forEach((b) => (b.disabled = false));
    loadStatus();
  }
});

const explorerNav = document.getElementById("explorer-nav");
if (explorerNav) explorerNav.href = buildExplorerUrl();

const uiHref = buildUiUrl();
const homeNav = document.getElementById("home-nav");
if (homeNav) homeNav.href = uiHref;
const brandLink = document.getElementById("brand-link");
if (brandLink) brandLink.href = uiHref;

loadStatus();
