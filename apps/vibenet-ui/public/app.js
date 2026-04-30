// base vibenet landing page bootstrap.
//
// Loads /config.json (UI content from etc/vibenet/config/vibenet.yaml) and
// /contracts.json (written by vibenet-setup), renders them, and wires up
// the chain-card utilities (click-to-copy values, Add to wallet). No build
// step, no bundler - one module, one stylesheet.
//
// The RPC is currently open (no API key path), so URL building is
// deterministic from the page's hostname.

import { createWalletClient, custom } from "https://esm.sh/viem@2.21.0";

// Hard-coded vibenet L2 chain id - matches L2_CHAIN_ID in vibenet-env. We
// surface it for display and for the wallet_addEthereumChain call below.
const VIBENET_CHAIN_ID = 84538453;

async function loadJson(url) {
  const res = await fetch(url, { cache: "no-store" });
  if (!res.ok) throw new Error(`${url} -> ${res.status}`);
  return res.json();
}

function isLocalHost(host) {
  return host === "localhost" || host === "127.0.0.1" || host === "0.0.0.0";
}

// Public hostname scheme (Cloudflare-fronted):
//   vibes.base.org            -> this UI
//   rpc.vibes.base.org        -> JSON-RPC + WS
//   explorer.vibes.base.org   -> vibescan
//   faucet.vibes.base.org     -> vibenet-faucet
//
// Local dev publishes the gateway on four sibling loopback ports
// (ui:18080, rpc:18081, explorer:18082, faucet:18083).
const PUBLIC_SERVICE_HOSTS = {
  rpc: "rpc.vibes.base.org",
  explorer: "explorer.vibes.base.org",
  faucet: "faucet.vibes.base.org",
};

function buildRpcUrl() {
  const host = location.hostname;
  if (isLocalHost(host)) {
    const uiPort = parseInt(location.port || "80", 10);
    const rpcPort = uiPort + 1;
    return `${location.protocol}//${host}:${rpcPort}`;
  }
  return `https://${PUBLIC_SERVICE_HOSTS.rpc}`;
}

function buildExplorerUrl() {
  const host = location.hostname;
  if (isLocalHost(host)) {
    const uiPort = parseInt(location.port || "80", 10);
    const explorerPort = uiPort + 2;
    return `${location.protocol}//${host}:${explorerPort}`;
  }
  return `https://${PUBLIC_SERVICE_HOSTS.explorer}`;
}

function buildFaucetUrl() {
  const host = location.hostname;
  if (isLocalHost(host)) {
    const uiPort = parseInt(location.port || "80", 10);
    const faucetPort = uiPort + 3;
    return `${location.protocol}//${host}:${faucetPort}`;
  }
  return `https://${PUBLIC_SERVICE_HOSTS.faucet}`;
}

async function main() {
  const [config, contracts] = await Promise.all([
    loadJson("/config.json").catch(() => ({})),
    loadJson("/contracts.json").catch(() => null),
  ]);

  document.getElementById("title").textContent = config.title || "base vibenet";
  document.getElementById("subtitle").textContent = config.subtitle || "";
  document.getElementById("branch").textContent = config.branch || "unknown";
  document.getElementById("commit").textContent = (config.commit || "unknown").slice(0, 12);

  const rpcUrl = buildRpcUrl();
  const explorerUrl = buildExplorerUrl();
  const faucetUrl = buildFaucetUrl();

  document.getElementById("chain-id").textContent = String(VIBENET_CHAIN_ID);
  document.getElementById("rpc-display").textContent = rpcUrl;

  const explorerEl = document.getElementById("explorer-link");
  explorerEl.href = explorerUrl;
  explorerEl.textContent = explorerUrl;

  const explorerNav = document.getElementById("explorer-nav");
  if (explorerNav) explorerNav.href = explorerUrl;

  const faucetNav = document.getElementById("faucet-nav");
  if (faucetNav) faucetNav.href = faucetUrl;

  renderFeatures(config.features || []);
  renderContracts(contracts, explorerUrl);

  wireAddToWallet(rpcUrl, explorerUrl);
  wireCopyableRows();
}

function renderFeatures(features) {
  const host = document.getElementById("features");
  host.innerHTML = "";
  if (!features.length) {
    host.innerHTML = `<p class="muted">No branch-specific features declared for this vibe.</p>`;
    return;
  }
  for (const f of features) {
    const card = document.createElement("div");
    card.className = "feature-card";

    const title = document.createElement("div");
    title.className = "feature-title";
    title.textContent = f.title || "";
    card.appendChild(title);

    if (f.description) {
      const desc = document.createElement("div");
      desc.className = "feature-desc";
      desc.textContent = f.description;
      card.appendChild(desc);
    }

    if (f.link) {
      const a = document.createElement("a");
      a.href = f.link;
      a.target = "_blank";
      a.rel = "noopener";
      a.textContent = "Learn more →";
      card.appendChild(a);
    }
    host.appendChild(card);
  }
}

// Friendly labels for the keys vibenet-setup writes into contracts.json.
// Anything not in the map falls back to the raw key. Keys starting with
// `_` are metadata and skipped entirely.
const CONTRACT_LABELS = {
  faucetAddress: "Faucet",
  usdv: "USDV (ERC-20)",
  nfv: "NFV (ERC-721)",
};

// Tokens we offer a wallet_watchAsset button for. Values here must match
// the on-chain `name()` / `symbol()` / `decimals()` so wallets that
// validate the metadata don't reject the prompt.
const WATCHABLE_TOKENS = {
  usdv: { type: "ERC20", symbol: "USDV", decimals: 6 },
};

function renderContracts(contracts, explorerBase) {
  const host = document.getElementById("contracts-list");
  if (!contracts) {
    host.innerHTML = `<p class="muted" style="padding: 0.75rem 1rem; margin: 0;">No contracts deployed on this vibe.</p>`;
    return;
  }
  host.innerHTML = "";
  let rendered = 0;
  for (const [k, v] of Object.entries(contracts)) {
    // `faucetAddress` is the faucet signer EOA, not a deployed contract.
    if (k === "faucetAddress") continue;
    if (k.startsWith("_")) continue;
    if (typeof v !== "string") continue;
    if (!/^0x[0-9a-fA-F]{40}$/.test(v)) continue;

    const row = document.createElement("div");
    row.className = "contract-row";

    const label = document.createElement("span");
    label.className = "contract-label";
    label.textContent = CONTRACT_LABELS[k] || k;

    const link = document.createElement("a");
    link.className = "contract-addr";
    link.href = `${explorerBase}/address/${v}`;
    link.target = "_blank";
    link.rel = "noopener";
    link.textContent = v;

    row.append(label, link);

    // If this contract is a known ERC-20 we can offer a "Watch in wallet"
    // button - clicking it calls wallet_watchAsset so the user sees their
    // token balance in their wallet UI without pasting addresses by hand.
    const meta = WATCHABLE_TOKENS[k];
    if (meta) {
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "watch-asset secondary small";
      btn.textContent = `Add ${meta.symbol} to wallet`;
      btn.addEventListener("click", () => watchAsset(v, meta, btn));
      row.append(btn);
    }

    host.appendChild(row);
    rendered++;
  }
  if (!rendered) {
    host.innerHTML = `<p class="muted" style="padding: 0.75rem 1rem; margin: 0;">No contracts deployed on this vibe.</p>`;
  }
}

// Fire wallet_watchAsset via viem so the user's wallet tracks the token.
// On success the wallet adds it to its token list; on user rejection /
// unsupported wallet we quietly surface the error on the button itself.
async function watchAsset(address, meta, btn) {
  const original = btn.textContent;
  const provider = window.ethereum;
  if (!provider) {
    btn.textContent = "No wallet detected";
    setTimeout(() => (btn.textContent = original), 1500);
    return;
  }
  try {
    const wallet = createWalletClient({ transport: custom(provider) });
    await wallet.watchAsset({
      type: meta.type,
      options: { address, symbol: meta.symbol, decimals: meta.decimals },
    });
    btn.textContent = `${meta.symbol} added`;
  } catch (err) {
    btn.textContent = err?.shortMessage || "Rejected";
  } finally {
    setTimeout(() => (btn.textContent = original), 1800);
  }
}

// Build a viem Chain object describing vibenet. Used by walletClient.addChain
// to trigger the wallet's native "Add network" prompt.
function vibenetChain(rpcUrl, explorerUrl) {
  return {
    id: VIBENET_CHAIN_ID,
    name: "base vibenet",
    nativeCurrency: { name: "Ether", symbol: "ETH", decimals: 18 },
    rpcUrls: { default: { http: [rpcUrl] } },
    blockExplorers: { default: { name: "vibescan", url: explorerUrl } },
  };
}

function wireAddToWallet(rpcUrl, explorerUrl) {
  const btn = document.getElementById("add-to-wallet");
  const status = document.getElementById("wallet-status");
  if (!btn) return;
  btn.addEventListener("click", async () => {
    const provider = window.ethereum;
    if (!provider) {
      status.textContent = "No browser wallet detected on this page.";
      return;
    }
    try {
      const wallet = createWalletClient({ transport: custom(provider) });
      await wallet.addChain({ chain: vibenetChain(rpcUrl, explorerUrl) });
      status.textContent = "Network added. Your wallet should now be on base vibenet.";
    } catch (err) {
      // User rejection is code 4001; surface everything else verbatim.
      status.textContent = `Wallet did not add the network: ${err?.shortMessage || err?.message || err}`;
    }
  });
}

// Wire click-to-copy on any element with [data-copy-target] pointing at the
// id of the <code> whose textContent should be copied. Shows a transient
// "Copied" affordance inside the button's hint span.
function wireCopyableRows() {
  const rows = document.querySelectorAll("[data-copy-target]");
  rows.forEach((row) => {
    row.addEventListener("click", async () => {
      const targetId = row.getAttribute("data-copy-target");
      const target = document.getElementById(targetId);
      if (!target) return;
      const hint = row.querySelector(".copy-hint");
      const originalHint = hint ? hint.textContent : "";
      try {
        await navigator.clipboard.writeText(target.textContent || "");
        if (hint) hint.textContent = "Copied";
        row.classList.add("copied");
      } catch {
        if (hint) hint.textContent = "Copy failed";
      }
      setTimeout(() => {
        if (hint) hint.textContent = originalHint;
        row.classList.remove("copied");
      }, 1200);
    });
  });
}

main().catch((err) => {
  document.body.insertAdjacentHTML(
    "afterbegin",
    `<pre style="color:#ff6b6b;padding:1rem;">Failed to load vibenet UI: ${err.message}</pre>`,
  );
});
