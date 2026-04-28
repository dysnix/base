use std::{
    fmt,
    fs::File,
    path::{Path, PathBuf},
    str::FromStr,
};

use alloy_chains::Chain;
use alloy_genesis::{ChainConfig as L1ChainConfig, Genesis as ExecutionGenesis};
use base_client_cli::{L1ConfigFile, L2ConfigFile};
use base_common_chains::ChainConfig as BuiltInChainConfig;
use base_common_genesis::RollupConfig;
use eyre::WrapErr;
use figment::{
    Figment,
    providers::{Env, Format, Serialized, Toml},
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Prefix for TOML configuration environment variable overrides.
pub(crate) const BASE_CONFIG_ENV_PREFIX: &str = "BASE_CONFIG_";

/// A built-in chain supported by the `base` binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum BuiltInChain {
    /// Base mainnet.
    Mainnet,
    /// Base sepolia.
    Sepolia,
    /// Base zeronet.
    Zeronet,
}

impl BuiltInChain {
    /// Returns the canonical CLI name for this chain.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Mainnet => "mainnet",
            Self::Sepolia => "sepolia",
            Self::Zeronet => "zeronet",
        }
    }

    /// Returns the built-in chain config backing this selection.
    pub(crate) const fn chain_config(self) -> &'static BuiltInChainConfig {
        match self {
            Self::Mainnet => BuiltInChainConfig::mainnet(),
            Self::Sepolia => BuiltInChainConfig::sepolia(),
            Self::Zeronet => BuiltInChainConfig::zeronet(),
        }
    }

    /// Returns the execution chain selector for this built-in chain.
    pub(crate) const fn execution_chain(self) -> &'static str {
        match self {
            Self::Mainnet => "base",
            Self::Sepolia => "base-sepolia",
            Self::Zeronet => "base-zeronet",
        }
    }

    /// Returns the built-in chain for a known L2 chain ID.
    pub(crate) const fn from_l2_chain_id(l2_chain_id: u64) -> Option<Self> {
        match l2_chain_id {
            8453 => Some(Self::Mainnet),
            84532 => Some(Self::Sepolia),
            763360 => Some(Self::Zeronet),
            _ => None,
        }
    }
}

impl fmt::Display for BuiltInChain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for BuiltInChain {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "mainnet" => Ok(Self::Mainnet),
            "sepolia" => Ok(Self::Sepolia),
            "zeronet" => Ok(Self::Zeronet),
            _ => Err(format!(
                "unsupported built-in chain `{value}`; expected one of mainnet, sepolia, zeronet"
            )),
        }
    }
}

/// CLI input for the root `--chain` flag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ChainArg {
    /// Use one of the built-in static chains.
    BuiltIn(BuiltInChain),
    /// Load chain settings from a TOML file.
    File(PathBuf),
}

impl Default for ChainArg {
    fn default() -> Self {
        Self::BuiltIn(BuiltInChain::Mainnet)
    }
}

impl FromStr for ChainArg {
    type Err = std::convert::Infallible;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(BuiltInChain::from_str(value)
            .map_or_else(|_| Self::File(PathBuf::from(value)), Self::BuiltIn))
    }
}

/// The concrete source of a resolved chain config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ResolvedChainSource {
    /// The config came from a built-in static chain.
    BuiltIn(BuiltInChain),
    /// The config came from a TOML file.
    File(PathBuf),
}

/// Execution-layer chain settings loaded from the chain TOML.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExecutionChainConfig {
    /// Optional path to an execution genesis JSON file.
    pub(crate) genesis_path: Option<PathBuf>,
    /// Optional sequencer HTTP URL for validator-side pending state and txpool forwarding.
    pub(crate) sequencer_url: Option<String>,
    /// Optional flashblocks websocket URL.
    pub(crate) flashblocks_url: Option<String>,
}

/// Consensus-layer chain settings loaded from the chain TOML.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConsensusChainConfig {
    /// Optional rollup config JSON path.
    pub(crate) rollup_config_path: Option<PathBuf>,
    /// Optional L1 chain config JSON path.
    pub(crate) l1_config_path: Option<PathBuf>,
    /// Optional fixed L1 slot duration override.
    pub(crate) l1_slot_duration_override: Option<u64>,
}

/// The resolved chain config used by the `base` binary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ResolvedChainConfig {
    /// Human-readable chain name.
    pub(crate) name: String,
    /// L2 chain ID.
    pub(crate) l2_chain_id: u64,
    /// L1 chain ID.
    pub(crate) l1_chain_id: u64,
    /// Optional execution chain selector or genesis path.
    pub(crate) execution_chain: Option<String>,
    /// Optional embedded execution genesis.
    ///
    /// This may be provided either as a TOML table/object or as a JSON string.
    pub(crate) execution_genesis: Option<serde_json::Value>,
    /// Optional rollup config JSON path.
    pub(crate) rollup_config_path: Option<PathBuf>,
    /// Optional embedded rollup config.
    ///
    /// This may be provided either as a TOML table/object or as a JSON string.
    pub(crate) rollup_config: Option<serde_json::Value>,
    /// Optional L1 chain config JSON path.
    pub(crate) l1_config_path: Option<PathBuf>,
    /// Optional embedded L1 chain config.
    ///
    /// This may be provided either as a TOML table/object or as a JSON string.
    pub(crate) l1_config: Option<serde_json::Value>,
    /// Optional fixed L1 slot duration override.
    pub(crate) l1_slot_duration_override: Option<u64>,
    /// Execution-layer chain configuration.
    #[serde(default)]
    pub(crate) execution: ExecutionChainConfig,
    /// Consensus-layer chain configuration.
    #[serde(default)]
    pub(crate) consensus: ConsensusChainConfig,
    /// Where this config came from.
    pub(crate) source: ResolvedChainSource,
}

impl ResolvedChainConfig {
    /// Creates a resolved config from merged values and an explicit source.
    pub(crate) fn new(values: ResolvedChainValues, source: ResolvedChainSource) -> Self {
        Self {
            name: values.name,
            l2_chain_id: values.l2_chain_id,
            l1_chain_id: values.l1_chain_id,
            execution_chain: values.execution_chain,
            execution_genesis: values.execution_genesis,
            rollup_config_path: values.rollup_config_path,
            rollup_config: values.rollup_config,
            l1_config_path: values.l1_config_path,
            l1_config: values.l1_config,
            l1_slot_duration_override: values.l1_slot_duration_override,
            execution: values.execution,
            consensus: values.consensus,
            source,
        }
    }

    /// Returns the execution chain selector or genesis path required to launch the EL.
    pub(crate) fn execution_chain_input(&self) -> eyre::Result<&str> {
        if let Some(chain) = self.execution_chain.as_deref() {
            return Ok(chain);
        }

        if let Some(path) = self.execution.genesis_path.as_ref() {
            return path.to_str().ok_or_else(|| {
                eyre::eyre!("execution genesis path is not valid UTF-8: {}", path.display())
            });
        }

        Self::infer_execution_chain(self.l2_chain_id).ok_or_else(|| {
            eyre::eyre!(
                "missing execution chain for L2 chain ID {}; set `execution_chain` or `execution.genesis_path` in the chain TOML",
                self.l2_chain_id
            )
        })
    }

    /// Resolves the execution chain input, materializing an embedded genesis file when present.
    pub(crate) fn prepare_execution_chain_input(
        &self,
        datadir: &BaseDatadir,
    ) -> eyre::Result<String> {
        if let Some(genesis) = self.load_execution_genesis()? {
            let path = datadir.execution_genesis_path();
            let file = File::create(&path)
                .wrap_err_with(|| format!("failed to create {}", path.display()))?;
            serde_json::to_writer_pretty(file, &genesis)
                .wrap_err_with(|| format!("failed to write {}", path.display()))?;
            return Ok(path.display().to_string());
        }

        self.execution_chain_input().map(str::to_owned)
    }

    /// Loads the embedded execution genesis when present.
    pub(crate) fn load_execution_genesis(&self) -> eyre::Result<Option<ExecutionGenesis>> {
        self.execution_genesis
            .as_ref()
            .map(|value| Self::decode_embedded_json(value, "execution_genesis"))
            .transpose()
    }

    /// Loads the rollup config from embedded data, an explicit JSON path, or the built-in chain.
    pub(crate) fn load_rollup_config(&self) -> eyre::Result<RollupConfig> {
        if let Some(value) = self.rollup_config.as_ref() {
            return Self::decode_embedded_json(value, "rollup_config");
        }

        let path = self
            .rollup_config_path
            .as_ref()
            .or(self.consensus.rollup_config_path.as_ref())
            .cloned();
        let l2_chain = Chain::from(self.l2_chain_id);
        L2ConfigFile::new(path)
            .load(&l2_chain)
            .map_err(|error| eyre::eyre!("{error}"))
            .wrap_err("failed to load rollup config")
    }

    /// Loads the L1 chain config from embedded data, an explicit JSON path, or the built-in map.
    pub(crate) fn load_l1_config(&self) -> eyre::Result<L1ChainConfig> {
        if let Some(value) = self.l1_config.as_ref() {
            return Self::decode_embedded_json(value, "l1_config");
        }

        let path = self.l1_config_path.as_ref().or(self.consensus.l1_config_path.as_ref()).cloned();

        L1ConfigFile::new(path)
            .load(self.l1_chain_id)
            .map_err(|error| eyre::eyre!("{error}"))
            .wrap_err("failed to load L1 chain config")
    }

    /// Returns the configured L1 slot duration override.
    pub(crate) const fn l1_slot_duration_override(&self) -> Option<u64> {
        match self.l1_slot_duration_override {
            Some(value) => Some(value),
            None => self.consensus.l1_slot_duration_override,
        }
    }

    /// Returns the configured sequencer URL.
    pub(crate) fn sequencer_url(&self) -> Option<&str> {
        self.execution.sequencer_url.as_deref()
    }

    /// Returns the configured flashblocks websocket URL.
    pub(crate) fn flashblocks_url(&self) -> eyre::Result<Option<url::Url>> {
        self.execution
            .flashblocks_url
            .as_ref()
            .map(|raw| {
                url::Url::parse(raw).wrap_err_with(|| {
                    format!("failed to parse `execution.flashblocks_url` URL `{raw}`")
                })
            })
            .transpose()
    }

    fn decode_embedded_json<T>(value: &serde_json::Value, field: &str) -> eyre::Result<T>
    where
        T: DeserializeOwned,
    {
        if let Some(raw) = value.as_str() {
            return serde_json::from_str(raw)
                .wrap_err_with(|| format!("failed to parse `{field}` JSON string"));
        }

        serde_json::from_value(value.clone())
            .wrap_err_with(|| format!("failed to parse `{field}` embedded config"))
    }

    fn infer_execution_chain(l2_chain_id: u64) -> Option<&'static str> {
        BuiltInChain::from_l2_chain_id(l2_chain_id).map(BuiltInChain::execution_chain)
    }
}

/// The subset of chain settings merged from built-ins, TOML, and env.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResolvedChainValues {
    /// Human-readable chain name.
    pub(crate) name: String,
    /// L2 chain ID.
    pub(crate) l2_chain_id: u64,
    /// L1 chain ID.
    pub(crate) l1_chain_id: u64,
    /// Optional execution chain selector or genesis path.
    pub(crate) execution_chain: Option<String>,
    /// Optional embedded execution genesis.
    ///
    /// This may be provided either as a TOML table/object or as a JSON string.
    pub(crate) execution_genesis: Option<serde_json::Value>,
    /// Optional rollup config JSON path.
    pub(crate) rollup_config_path: Option<PathBuf>,
    /// Optional embedded rollup config.
    ///
    /// This may be provided either as a TOML table/object or as a JSON string.
    pub(crate) rollup_config: Option<serde_json::Value>,
    /// Optional L1 chain config JSON path.
    pub(crate) l1_config_path: Option<PathBuf>,
    /// Optional embedded L1 chain config.
    ///
    /// This may be provided either as a TOML table/object or as a JSON string.
    pub(crate) l1_config: Option<serde_json::Value>,
    /// Optional fixed L1 slot duration override.
    pub(crate) l1_slot_duration_override: Option<u64>,
    /// Execution-layer chain configuration.
    #[serde(default)]
    pub(crate) execution: ExecutionChainConfig,
    /// Consensus-layer chain configuration.
    #[serde(default)]
    pub(crate) consensus: ConsensusChainConfig,
}

impl ResolvedChainValues {
    /// Creates resolved values from a built-in chain.
    pub(crate) fn from_builtin(chain: BuiltInChain) -> Self {
        let config = chain.chain_config();
        Self {
            name: chain.as_str().to_owned(),
            l2_chain_id: config.chain_id,
            l1_chain_id: config.l1_chain_id,
            execution_chain: Some(chain.execution_chain().to_owned()),
            execution_genesis: None,
            rollup_config_path: None,
            rollup_config: None,
            l1_config_path: None,
            l1_config: None,
            l1_slot_duration_override: None,
            execution: ExecutionChainConfig::default(),
            consensus: ConsensusChainConfig::default(),
        }
    }

    /// Validates the required chain identity fields.
    pub(crate) fn validate(&self) -> eyre::Result<()> {
        if self.name.trim().is_empty() {
            eyre::bail!("chain config is missing a non-empty `name`");
        }
        if self.l2_chain_id == 0 {
            eyre::bail!("chain config is missing a non-zero `l2_chain_id`");
        }
        if self.l1_chain_id == 0 {
            eyre::bail!("chain config is missing a non-zero `l1_chain_id`");
        }
        Ok(())
    }
}

/// Resolves a chain selection into a concrete config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChainResolver {
    /// The requested chain input.
    chain: ChainArg,
}

impl ChainResolver {
    /// Creates a new chain resolver.
    pub(crate) const fn new(chain: ChainArg) -> Self {
        Self { chain }
    }

    /// Resolves the configured chain input.
    pub(crate) fn resolve(&self) -> eyre::Result<ResolvedChainConfig> {
        match &self.chain {
            ChainArg::BuiltIn(chain) => {
                let figment =
                    Figment::from(Serialized::defaults(ResolvedChainValues::from_builtin(*chain)))
                        .merge(Self::env_provider());
                Self::extract(figment, ResolvedChainSource::BuiltIn(*chain))
            }
            ChainArg::File(path) => Self::resolve_file(path),
        }
    }

    /// Resolves a chain config from a TOML file.
    pub(crate) fn resolve_file(path: &Path) -> eyre::Result<ResolvedChainConfig> {
        let figment = Figment::new().merge(Toml::file(path)).merge(Self::env_provider());
        Self::extract(figment, ResolvedChainSource::File(path.to_path_buf()))
    }

    /// Returns the environment provider for TOML configuration overrides.
    pub(crate) fn env_provider() -> Env {
        Env::raw().filter_map(|key| {
            key.as_str().strip_prefix(BASE_CONFIG_ENV_PREFIX).map(|name| {
                let name = name.to_ascii_lowercase();
                Self::env_override_key(&name).into()
            })
        })
    }

    fn env_override_key(name: &str) -> String {
        match name {
            "execution_chain" | "execution_genesis" => name.to_owned(),
            _ => name.strip_prefix("execution_").map_or_else(
                || {
                    name.strip_prefix("consensus_")
                        .map_or_else(|| name.to_owned(), |field| format!("consensus.{field}"))
                },
                |field| format!("execution.{field}"),
            ),
        }
    }

    /// Extracts the merged chain values into the public resolved config.
    pub(crate) fn extract(
        figment: Figment,
        source: ResolvedChainSource,
    ) -> eyre::Result<ResolvedChainConfig> {
        let values = figment.extract::<ResolvedChainValues>().wrap_err_with(|| match &source {
            ResolvedChainSource::BuiltIn(chain) => {
                format!("failed to resolve chain config for built-in chain `{chain}`")
            }
            ResolvedChainSource::File(path) => {
                format!("failed to resolve chain config from {}", path.display())
            }
        })?;
        values.validate()?;

        Ok(ResolvedChainConfig::new(values, source))
    }
}

/// Datadir layout for the unified node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BaseDatadir {
    /// Root datadir path.
    pub(crate) path: PathBuf,
}

impl BaseDatadir {
    /// Creates a new datadir wrapper.
    pub(crate) const fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Returns the default datadir for the given chain name.
    pub(crate) fn default_for_chain(name: &str) -> eyre::Result<Self> {
        let home =
            dirs::home_dir().ok_or_else(|| eyre::eyre!("failed to resolve home directory"))?;
        Ok(Self::new(home.join(".base").join(name)))
    }

    /// Ensures the root datadir exists.
    pub(crate) fn ensure(&self) -> eyre::Result<()> {
        std::fs::create_dir_all(&self.path)
            .wrap_err_with(|| format!("failed to create datadir {}", self.path.display()))
    }

    /// Returns the MDBX database path.
    pub(crate) fn db_path(&self) -> PathBuf {
        self.path.join("db")
    }

    /// Returns the materialized execution genesis path.
    pub(crate) fn execution_genesis_path(&self) -> PathBuf {
        self.path.join("base-genesis.json")
    }

    /// Returns the persistent CL peer key path.
    pub(crate) fn p2p_key_path(&self) -> PathBuf {
        self.path.join("base-p2p-key.txt")
    }

    /// Returns the persistent safe head database path.
    pub(crate) fn safedb_path(&self) -> PathBuf {
        self.path.join("base-safedb.redb")
    }

    /// Returns the EL JWT secret path.
    pub(crate) fn jwt_secret_path(&self) -> PathBuf {
        self.path.join("jwt.hex")
    }

    /// Writes the provided JWT secret file content if it does not already exist.
    pub(crate) fn ensure_jwt_secret(&self) -> eyre::Result<()> {
        let jwt_path = self.jwt_secret_path();
        if jwt_path.exists() {
            return Ok(());
        }

        alloy_rpc_types_engine::JwtSecret::try_create_random(&jwt_path)
            .map(|_| ())
            .wrap_err_with(|| format!("failed to write JWT secret {}", jwt_path.display()))
    }

    /// Returns the datadir path converted to an absolute path.
    pub(crate) fn canonicalized(&self) -> eyre::Result<Self> {
        let path = self
            .path
            .canonicalize()
            .wrap_err_with(|| format!("failed to canonicalize datadir {}", self.path.display()))?;
        Ok(Self::new(path))
    }
}

#[cfg(test)]
mod tests {
    use alloy_eips::eip1898::BlockNumHash;
    use alloy_primitives::{Address, B256, U256};
    use figment::Jail;

    use super::*;
    use base_common_genesis::{ChainGenesis, FeeConfig};

    fn with_cleared_env(test: impl FnOnce(&mut Jail) -> figment::Result<()>) {
        Jail::expect_with(|jail| {
            jail.clear_env();
            test(jail)
        });
    }

    #[test]
    fn resolves_mainnet_builtin() {
        with_cleared_env(|_| {
            let resolved =
                ChainResolver::new(ChainArg::BuiltIn(BuiltInChain::Mainnet)).resolve().unwrap();

            assert_eq!(resolved.name, "mainnet");
            assert_eq!(resolved.l2_chain_id, 8453);
            assert_eq!(resolved.l1_chain_id, 1);
            assert_eq!(resolved.execution_chain_input().unwrap(), "base");
            assert_eq!(resolved.sequencer_url(), None);
            assert_eq!(resolved.source, ResolvedChainSource::BuiltIn(BuiltInChain::Mainnet));

            Ok(())
        });
    }

    #[test]
    fn resolves_sepolia_builtin() {
        with_cleared_env(|_| {
            let resolved =
                ChainResolver::new(ChainArg::BuiltIn(BuiltInChain::Sepolia)).resolve().unwrap();

            assert_eq!(resolved.name, "sepolia");
            assert_eq!(resolved.l2_chain_id, 84532);
            assert_eq!(resolved.l1_chain_id, 11155111);
            assert_eq!(resolved.execution_chain_input().unwrap(), "base-sepolia");

            Ok(())
        });
    }

    #[test]
    fn resolves_zeronet_builtin() {
        with_cleared_env(|_| {
            let resolved =
                ChainResolver::new(ChainArg::BuiltIn(BuiltInChain::Zeronet)).resolve().unwrap();

            assert_eq!(resolved.name, "zeronet");
            assert_eq!(resolved.l2_chain_id, 763360);
            assert_eq!(resolved.source, ResolvedChainSource::BuiltIn(BuiltInChain::Zeronet));

            Ok(())
        });
    }

    #[test]
    fn resolves_custom_toml_file() {
        with_cleared_env(|jail| {
            let path = jail.directory().join("chain.toml");
            jail.create_file(
                &path,
                concat!(
                    "name = \"custom-chain\"\n",
                    "l2_chain_id = 999\n",
                    "l1_chain_id = 11155111\n",
                    "execution_chain = \"/tmp/genesis.json\"\n",
                    "rollup_config_path = \"/tmp/rollup.json\"\n",
                    "l1_config_path = \"/tmp/l1.json\"\n",
                    "l1_slot_duration_override = 4\n",
                    "\n",
                    "[execution]\n",
                    "genesis_path = \"/tmp/nested-genesis.json\"\n",
                    "sequencer_url = \"http://sequencer.example\"\n",
                    "flashblocks_url = \"ws://flashblocks.example\"\n",
                    "\n",
                    "[consensus]\n",
                    "rollup_config_path = \"/tmp/nested-rollup.json\"\n",
                    "l1_config_path = \"/tmp/nested-l1.json\"\n",
                    "l1_slot_duration_override = 8\n",
                ),
            )?;

            let resolved = ChainResolver::resolve_file(&path).unwrap();

            assert_eq!(resolved.name, "custom-chain");
            assert_eq!(resolved.l2_chain_id, 999);
            assert_eq!(resolved.l1_chain_id, 11155111);
            assert_eq!(resolved.execution_chain_input().unwrap(), "/tmp/genesis.json");
            assert_eq!(resolved.rollup_config_path, Some(PathBuf::from("/tmp/rollup.json")));
            assert_eq!(resolved.l1_config_path, Some(PathBuf::from("/tmp/l1.json")));
            assert_eq!(resolved.l1_slot_duration_override, Some(4));
            assert_eq!(resolved.sequencer_url(), Some("http://sequencer.example"));
            assert_eq!(
                resolved.flashblocks_url().unwrap().map(|url| url.to_string()),
                Some("ws://flashblocks.example/".to_owned())
            );
            assert_eq!(resolved.execution_chain_input().unwrap(), "/tmp/genesis.json");
            assert_eq!(
                resolved.execution.genesis_path,
                Some(PathBuf::from("/tmp/nested-genesis.json"))
            );
            assert_eq!(
                resolved.execution.sequencer_url.as_deref(),
                Some("http://sequencer.example")
            );
            assert_eq!(
                resolved.execution.flashblocks_url.as_deref(),
                Some("ws://flashblocks.example")
            );
            assert_eq!(
                resolved.consensus.rollup_config_path,
                Some(PathBuf::from("/tmp/nested-rollup.json"))
            );
            assert_eq!(
                resolved.consensus.l1_config_path,
                Some(PathBuf::from("/tmp/nested-l1.json"))
            );
            assert_eq!(resolved.l1_slot_duration_override(), Some(4));
            assert_eq!(resolved.source, ResolvedChainSource::File(path));

            Ok(())
        });
    }

    #[test]
    fn base_config_env_overrides_custom_toml_file() {
        with_cleared_env(|jail| {
            let path = jail.directory().join("chain.toml");
            jail.create_file(
                &path,
                concat!(
                    "name = \"custom-chain\"\n",
                    "l2_chain_id = 999\n",
                    "l1_chain_id = 11155111\n",
                    "l1_slot_duration_override = 4\n",
                    "\n",
                    "[execution]\n",
                    "genesis_path = \"/tmp/toml-genesis.json\"\n",
                    "sequencer_url = \"http://toml-sequencer.example\"\n",
                    "flashblocks_url = \"ws://toml-flashblocks.example\"\n",
                    "\n",
                    "[consensus]\n",
                    "rollup_config_path = \"/tmp/toml-rollup.json\"\n",
                ),
            )?;
            jail.set_env("BASE_CONFIG_L1_SLOT_DURATION_OVERRIDE", "8");
            jail.set_env("BASE_CONFIG_EXECUTION_SEQUENCER_URL", "http://env-sequencer.example");
            jail.set_env("BASE_CONFIG_EXECUTION_FLASHBLOCKS_URL", "ws://env-flashblocks.example");
            jail.set_env("BASE_CONFIG_EXECUTION_GENESIS_PATH", "/tmp/env-genesis.json");
            jail.set_env("BASE_CONFIG_CONSENSUS_ROLLUP_CONFIG_PATH", "/tmp/env-rollup.json");

            let resolved = ChainResolver::resolve_file(&path).unwrap();

            assert_eq!(resolved.l1_slot_duration_override, Some(8));
            assert_eq!(resolved.sequencer_url(), Some("http://env-sequencer.example"));
            assert_eq!(
                resolved.flashblocks_url().unwrap().map(|url| url.to_string()),
                Some("ws://env-flashblocks.example/".to_owned())
            );
            assert_eq!(
                resolved.execution.genesis_path,
                Some(PathBuf::from("/tmp/env-genesis.json"))
            );
            assert_eq!(
                resolved.consensus.rollup_config_path,
                Some(PathBuf::from("/tmp/env-rollup.json"))
            );

            Ok(())
        });
    }

    #[test]
    fn base_config_env_preserves_top_level_execution_fields() {
        with_cleared_env(|jail| {
            let path = jail.directory().join("chain.toml");
            jail.create_file(
                &path,
                "name = \"custom-chain\"\nl2_chain_id = 999\nl1_chain_id = 11155111\n",
            )?;
            jail.set_env("BASE_CONFIG_EXECUTION_CHAIN", "base-custom");

            let resolved = ChainResolver::resolve_file(&path).unwrap();

            assert_eq!(resolved.execution_chain, Some("base-custom".to_owned()));
            assert_eq!(resolved.execution_chain_input().unwrap(), "base-custom");

            Ok(())
        });
    }

    #[test]
    fn base_config_env_preserves_execution_prefixed_top_level_keys() {
        assert_eq!(ChainResolver::env_override_key("execution_chain"), "execution_chain");
        assert_eq!(ChainResolver::env_override_key("execution_genesis"), "execution_genesis");
        assert_eq!(
            ChainResolver::env_override_key("execution_genesis_path"),
            "execution.genesis_path"
        );
        assert_eq!(
            ChainResolver::env_override_key("consensus_rollup_config_path"),
            "consensus.rollup_config_path"
        );
    }

    #[test]
    fn rejects_operator_runtime_values_in_custom_toml_file() {
        with_cleared_env(|jail| {
            let path = jail.directory().join("chain.toml");
            jail.create_file(
                &path,
                concat!(
                    "name = \"custom-chain\"\n",
                    "l2_chain_id = 999\n",
                    "l1_chain_id = 11155111\n",
                    "\n",
                    "[execution]\n",
                    "datadir = \"/tmp/base-data\"\n",
                ),
            )?;

            let error = ChainResolver::resolve_file(&path).unwrap_err();
            let rendered = format!("{error:?}");

            assert!(rendered.contains("datadir"));

            Ok(())
        });
    }

    #[test]
    fn rejects_top_level_execution_urls_in_custom_toml_file() {
        with_cleared_env(|jail| {
            let path = jail.directory().join("chain.toml");
            jail.create_file(
                &path,
                concat!(
                    "name = \"custom-chain\"\n",
                    "l2_chain_id = 999\n",
                    "l1_chain_id = 11155111\n",
                    "sequencer_url = \"http://sequencer.example\"\n",
                ),
            )?;

            let error = ChainResolver::resolve_file(&path).unwrap_err();
            let rendered = format!("{error:?}");

            assert!(rendered.contains("sequencer_url"));

            Ok(())
        });
    }

    #[test]
    fn rejects_zero_l2_chain_id() {
        with_cleared_env(|jail| {
            let path = jail.directory().join("chain.toml");
            jail.create_file(&path, "name = \"custom-chain\"\nl2_chain_id = 0\nl1_chain_id = 1\n")?;

            let error = ChainResolver::resolve_file(&path).unwrap_err();

            assert!(error.to_string().contains("missing a non-zero `l2_chain_id`"));

            Ok(())
        });
    }

    #[test]
    fn infers_known_execution_chain_from_file_without_override() {
        with_cleared_env(|jail| {
            let path = jail.directory().join("chain.toml");
            jail.create_file(
                &path,
                "name = \"sepolia\"\nl2_chain_id = 84532\nl1_chain_id = 11155111\n",
            )?;

            let resolved = ChainResolver::resolve_file(&path).unwrap();

            assert_eq!(resolved.execution_chain_input().unwrap(), "base-sepolia");

            Ok(())
        });
    }

    #[test]
    fn resolves_custom_toml_with_embedded_configs() {
        with_cleared_env(|jail| {
            let path = jail.directory().join("chain.toml");
            let execution_genesis = serde_json::to_string(&ExecutionGenesis {
                config: L1ChainConfig { chain_id: 999, ..Default::default() },
                ..Default::default()
            })
            .unwrap();
            let rollup_config = serde_json::to_string(&RollupConfig {
                genesis: ChainGenesis {
                    l1: BlockNumHash { number: 0, hash: B256::ZERO },
                    l2: BlockNumHash { number: 0, hash: B256::ZERO },
                    l2_time: 0,
                    system_config: None,
                },
                block_time: 2,
                max_sequencer_drift: 600,
                seq_window_size: 120,
                channel_timeout: 120,
                granite_channel_timeout: 120,
                l1_chain_id: 1337,
                l2_chain_id: Chain::from(999_u64),
                hardforks: Default::default(),
                batch_inbox_address: Address::ZERO,
                deposit_contract_address: Address::ZERO,
                l1_system_config_address: Address::ZERO,
                protocol_versions_address: Address::ZERO,
                blobs_enabled_l1_timestamp: None,
                chain_op_config: FeeConfig::base_mainnet(),
            })
            .unwrap();
            let l1_config = serde_json::to_string(&L1ChainConfig {
                chain_id: 1337,
                terminal_total_difficulty: Some(U256::ZERO),
                ..Default::default()
            })
            .unwrap();

            jail.create_file(
                &path,
                &format!(
                    concat!(
                        "name = \"custom-chain\"\n",
                        "l2_chain_id = 999\n",
                        "l1_chain_id = 1337\n",
                        "execution_genesis = '''{execution_genesis}'''\n",
                        "rollup_config = '''{rollup_config}'''\n",
                        "l1_config = '''{l1_config}'''\n",
                    ),
                    execution_genesis = execution_genesis,
                    rollup_config = rollup_config,
                    l1_config = l1_config,
                ),
            )?;

            let resolved = ChainResolver::resolve_file(&path).unwrap();

            assert_eq!(resolved.load_execution_genesis().unwrap().unwrap().config.chain_id, 999);
            assert_eq!(resolved.load_rollup_config().unwrap().l2_chain_id.id(), 999);
            assert_eq!(resolved.load_l1_config().unwrap().chain_id, 1337);

            Ok(())
        });
    }

    #[test]
    fn default_datadir_uses_home_directory() {
        let home = dirs::home_dir().expect("home directory should be available for tests");
        let datadir = BaseDatadir::default_for_chain("devnet").unwrap();

        assert_eq!(datadir.path, home.join(".base").join("devnet"));
    }
}
