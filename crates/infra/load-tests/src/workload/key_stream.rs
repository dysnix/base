use alloy_signer_local::{MnemonicBuilder, PrivateKeySigner, coins_bip39::English};
use rand::{Rng, SeedableRng, rngs::StdRng};

use crate::utils::{BaselineError, Result};

/// Lazy stream of `secp256k1` signing keys used for sender pool generation
/// and on-demand recipient generation in fresh-recipient mode.
///
/// Both variants advance one key per [`KeyStream::next_signer`] call, so a
/// caller that pre-skips `offset` keys and then takes `n` keys produces the
/// same sequence as `AccountPool::with_offset(_, n, offset)` /
/// `AccountPool::from_mnemonic(_, n, offset)`. This is the contract that lets
/// users recover recipient addresses out-of-band.
#[derive(Debug)]
pub enum KeyStream {
    /// `StdRng`-driven derivation: each `next_signer` consumes 32 bytes.
    /// Boxed because `StdRng` is ~256 bytes and dwarfs the other variant.
    Seed(Box<StdRng>),
    /// BIP39 derivation: each `next_signer` advances `next_index`.
    Mnemonic {
        /// BIP39 phrase used to derive each key.
        phrase: String,
        /// BIP39 child index for the next [`KeyStream::next_signer`] call.
        next_index: u32,
    },
}

impl KeyStream {
    /// Builds a seed-driven stream positioned `offset` keys in. Each skipped
    /// position consumes 32 bytes from the underlying RNG, matching
    /// `AccountPool::with_offset`.
    pub fn from_seed(seed: u64, offset: usize) -> Self {
        let mut rng = StdRng::seed_from_u64(seed);
        for _ in 0..offset {
            let mut skip = [0u8; 32];
            rng.fill(&mut skip);
        }
        Self::Seed(Box::new(rng))
    }

    /// Builds a mnemonic-driven stream positioned at BIP39 index `offset`.
    pub fn from_mnemonic(phrase: impl Into<String>, offset: usize) -> Result<Self> {
        let next_index = u32::try_from(offset).map_err(|_| {
            BaselineError::Config(format!("mnemonic index {offset} exceeds u32::MAX"))
        })?;
        Ok(Self::Mnemonic { phrase: phrase.into(), next_index })
    }

    /// Yields the next signer in the stream.
    ///
    /// For `Seed`, the (vanishingly rare) case of an invalid secp256k1 scalar
    /// is handled by drawing again. For `Mnemonic`, returns an error if the
    /// next index would overflow `u32::MAX` or if BIP39 derivation fails.
    pub fn next_signer(&mut self) -> Result<PrivateKeySigner> {
        match self {
            Self::Seed(rng) => loop {
                let mut bytes = [0u8; 32];
                rng.fill(&mut bytes);
                if let Ok(signer) = PrivateKeySigner::from_bytes(&bytes.into()) {
                    return Ok(signer);
                }
            },
            Self::Mnemonic { phrase, next_index } => {
                let index = *next_index;
                let signer = MnemonicBuilder::<English>::default()
                    .phrase(phrase.as_str())
                    .index(index)
                    .map_err(|e| {
                        BaselineError::Config(format!("invalid mnemonic index {index}: {e}"))
                    })?
                    .build()
                    .map_err(|e| BaselineError::Config(format!("failed to derive key: {e}")))?;
                *next_index = next_index.checked_add(1).ok_or_else(|| {
                    BaselineError::Config(
                        "mnemonic index would overflow u32::MAX after derivation".into(),
                    )
                })?;
                Ok(signer)
            }
        }
    }
}
