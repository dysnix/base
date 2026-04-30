# base-vibenet-explorer

`base-vibenet-explorer` is a minimal block explorer for vibenet.

The upstream node already answers read-by-hash and read-by-number queries over
JSON-RPC. The explorer persists only the address activity index that the node
cannot serve directly. Block bodies, receipts, logs, balances, code, and
storage are fetched from the upstream RPC on demand so the explorer stays thin
and easy to reset with the devnet.
