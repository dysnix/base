# `base`

Unified Base validator binary.

`base` starts the execution layer and consensus layer in a single process for
validator use-cases. In this initial pass:

- the embedded EL serves public HTTP RPC on `8545` and WebSocket RPC on `8546`
- the embedded EL ports can be overridden with `--el.http-port` and `--el.ws-port`
- the embedded CL serves rollup RPC on `9545`
- EL/CL engine communication is wired over auth IPC only
- the default datadir is `~/.base/<chain-name>` unless `--datadir` or
  `BASE_DATADIR` is set
- operator runtime settings such as datadir, listener ports, P2P peers, and
  metrics are configured by CLI flags or their matching environment variables
- custom chain TOML values can be overridden with `BASE_CONFIG_` environment
  variables, such as `BASE_CONFIG_EXECUTION_SEQUENCER_URL`

Supported CLI forms:

```text
base node --l1-eth-rpc <url> --l1-beacon <url>
base node --flavor rpc --l1-eth-rpc <url> --l1-beacon <url>
base --chain sepolia node --l1-eth-rpc <url> --l1-beacon <url>
base -c sepolia node --l1-eth-rpc <url> --l1-beacon <url>
base --chain ./chain.toml node --l1-eth-rpc <url> --l1-beacon <url>
base -c ./chain.toml node --l1-eth-rpc <url> --l1-beacon <url>
```

Chain selection supports:

- built-in names: `mainnet`, `sepolia`, `zeronet`
- TOML files for custom chains

Custom chain TOML files may include the unified launcher inputs using JSON file
paths.

```toml
name = "devnet"
l2_chain_id = 84538453
l1_chain_id = 1337

[execution]
genesis_path = "/path/to/genesis.json"
sequencer_url = "https://sequencer.example"
flashblocks_url = "wss://flashblocks.example"

[consensus]
rollup_config_path = "/path/to/rollup.json"
l1_config_path = "/path/to/l1-chain-config.json"
l1_slot_duration_override = 4
```
