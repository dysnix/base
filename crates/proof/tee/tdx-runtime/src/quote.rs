use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
};

use alloy_primitives::Bytes;

use crate::{Result, TdxReportData, TdxRuntimeError};

/// Default Linux TSM/configfs report root.
pub const DEFAULT_TSM_REPORT_ROOT: &str = "/sys/kernel/config/tsm/report";

/// Provider name exposed by the Linux TDX guest TSM backend.
pub const TDX_CONFIGFS_PROVIDER_NAME: &str = "tdx_guest";

const INBLOB_FILE: &str = "inblob";
const OUTBLOB_FILE: &str = "outblob";
const AUXBLOB_FILE: &str = "auxblob";
const GENERATION_FILE: &str = "generation";
const PROVIDER_FILE: &str = "provider";

static CONFIGFS_REPORT_DIR_LOCKS: OnceLock<Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>> =
    OnceLock::new();

/// Local metadata emitted while collecting a TDX quote.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TdxLocalQuoteMetadata {
    /// Provider identifier used for quote generation.
    pub provider: String,
    /// Optional TSM auxiliary blob returned next to the quote.
    pub aux_blob: Option<Bytes>,
}

/// Raw quote bytes plus provider-local metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TdxCollectedQuote {
    /// Raw TDX quote bytes.
    pub quote: Bytes,
    /// Quote-generation metadata that may be needed by verifier input builders.
    pub metadata: TdxLocalQuoteMetadata,
}

/// Narrow provider trait for TDX quote generation.
pub trait TdxQuoteProvider: Send + Sync {
    /// Generates a quote over exactly 64 report-data bytes.
    fn quote(&self, report_data: &[u8]) -> Result<TdxCollectedQuote>;
}

/// TDX quote provider backed by Linux TSM/configfs.
#[derive(Clone, Debug)]
pub struct ConfigfsTdxQuoteProvider {
    report_dir: PathBuf,
    quote_lock: Arc<Mutex<()>>,
}

impl ConfigfsTdxQuoteProvider {
    /// Creates a provider under the default TSM report root.
    pub fn new(report_name: impl AsRef<Path>) -> Self {
        Self::with_report_dir(Path::new(DEFAULT_TSM_REPORT_ROOT).join(report_name))
    }

    /// Creates a provider from a concrete report directory.
    pub fn with_report_dir(report_dir: impl Into<PathBuf>) -> Self {
        let report_dir = report_dir.into();
        let locks = CONFIGFS_REPORT_DIR_LOCKS.get_or_init(Mutex::default);
        let mut locks = locks.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let quote_lock = locks.get(&report_dir).and_then(Weak::upgrade).unwrap_or_else(|| {
            locks.retain(|_, weak| weak.strong_count() > 0);
            let quote_lock = Arc::new(Mutex::new(()));
            locks.insert(report_dir.clone(), Arc::downgrade(&quote_lock));
            quote_lock
        });

        Self { report_dir, quote_lock }
    }

    /// Reads the optional TSM auxiliary blob if the provider exposes one.
    pub fn read_optional_aux_blob(&self) -> Result<Option<Bytes>> {
        let aux_path = self.report_dir.join(AUXBLOB_FILE);
        match fs::read(&aux_path) {
            Ok(bytes) if bytes.is_empty() => Ok(None),
            Ok(bytes) => Ok(Some(Bytes::from(bytes))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(TdxRuntimeError::filesystem_at(&aux_path, error)),
        }
    }

    /// Reads the TSM report generation counter.
    pub fn read_generation(&self) -> Result<u64> {
        let generation_path = self.report_dir.join(GENERATION_FILE);
        let generation = fs::read_to_string(&generation_path)
            .map_err(|error| TdxRuntimeError::filesystem_at(&generation_path, error))?;

        generation.trim().parse::<u64>().map_err(|error| {
            TdxRuntimeError::QuoteGeneration(format!(
                "invalid configfs generation at {}: {error}",
                generation_path.display()
            ))
        })
    }

    /// Verifies that the TSM report generation counter still matches this request.
    pub fn verify_generation(&self, expected_generation: u64) -> Result<()> {
        let actual_generation = self.read_generation()?;
        if actual_generation == expected_generation {
            return Ok(());
        }

        Err(TdxRuntimeError::ConfigfsGenerationMismatch {
            expected: expected_generation,
            actual: actual_generation,
        })
    }

    /// Verifies that the configfs provider marker is TDX when present.
    pub fn verify_provider(&self) -> Result<()> {
        let provider_path = self.report_dir.join(PROVIDER_FILE);
        match fs::read_to_string(&provider_path) {
            Ok(provider) if provider.trim() == TDX_CONFIGFS_PROVIDER_NAME => Ok(()),
            Ok(provider) => {
                Err(TdxRuntimeError::UnexpectedConfigfsProvider(provider.trim().to_owned()))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(TdxRuntimeError::filesystem_at(&provider_path, error)),
        }
    }
}

impl TdxQuoteProvider for ConfigfsTdxQuoteProvider {
    fn quote(&self, report_data: &[u8]) -> Result<TdxCollectedQuote> {
        TdxReportData::validate(report_data)?;
        let _quote_guard = self.quote_lock.lock().map_err(|_| {
            TdxRuntimeError::QuoteGeneration("configfs quote lock is poisoned".into())
        })?;

        fs::create_dir_all(&self.report_dir)
            .map_err(|error| TdxRuntimeError::filesystem_at(&self.report_dir, error))?;
        self.verify_provider()?;
        let expected_generation = self.read_generation()?.checked_add(1).ok_or_else(|| {
            TdxRuntimeError::QuoteGeneration(
                "configfs generation counter overflowed while collecting a quote".into(),
            )
        })?;

        let inblob_path = self.report_dir.join(INBLOB_FILE);
        fs::write(&inblob_path, report_data)
            .map_err(|error| TdxRuntimeError::filesystem_at(&inblob_path, error))?;

        let outblob_path = self.report_dir.join(OUTBLOB_FILE);
        let quote = fs::read(&outblob_path)
            .map_err(|error| TdxRuntimeError::filesystem_at(&outblob_path, error))?;
        if quote.is_empty() {
            return Err(TdxRuntimeError::QuoteGeneration(
                "configfs returned an empty quote".into(),
            ));
        }
        let aux_blob = self.read_optional_aux_blob()?;
        self.verify_generation(expected_generation)?;

        Ok(TdxCollectedQuote {
            quote: Bytes::from(quote),
            metadata: TdxLocalQuoteMetadata {
                provider: TDX_CONFIGFS_PROVIDER_NAME.to_owned(),
                aux_blob,
            },
        })
    }
}

/// Deterministic quote provider for local tests and CI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MockTdxQuoteProvider {
    quote: Bytes,
}

impl MockTdxQuoteProvider {
    /// Creates a deterministic mock provider returning the supplied fixture quote.
    pub fn new(quote: impl Into<Bytes>) -> Self {
        Self { quote: quote.into() }
    }
}

impl TdxQuoteProvider for MockTdxQuoteProvider {
    fn quote(&self, report_data: &[u8]) -> Result<TdxCollectedQuote> {
        TdxReportData::validate(report_data)?;
        Ok(TdxCollectedQuote {
            quote: self.quote.clone(),
            metadata: TdxLocalQuoteMetadata { provider: "mock".to_owned(), aux_blob: None },
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::Write,
        path::{Path, PathBuf},
        process::Command,
        thread::{self, JoinHandle},
    };

    use alloy_primitives::Bytes;
    use tempfile::TempDir;

    use super::*;
    use crate::TDX_REPORT_DATA_LEN;

    fn create_generation_fifo(report_dir: &Path) -> PathBuf {
        let generation_path = report_dir.join("generation");
        let status = Command::new("mkfifo").arg(&generation_path).status().unwrap();
        assert!(status.success());
        generation_path
    }

    fn spawn_generation_writer(
        generation_path: &Path,
        generations: impl IntoIterator<Item = u64>,
    ) -> JoinHandle<()> {
        let generation_path = generation_path.to_path_buf();
        let generations = generations.into_iter().collect::<Vec<_>>();

        thread::spawn(move || {
            for generation in generations {
                let mut file = fs::OpenOptions::new().write(true).open(&generation_path).unwrap();
                writeln!(file, "{generation}").unwrap();
            }
        })
    }

    #[test]
    fn mock_provider_returns_fixture_quote_without_hardware() {
        let fixture = Bytes::from_static(b"fixture-tdx-quote");
        let provider = MockTdxQuoteProvider::new(fixture.clone());
        let collected = provider.quote(&[0xA5; TDX_REPORT_DATA_LEN]).unwrap();

        assert_eq!(collected.quote, fixture);
        assert_eq!(collected.metadata.provider, "mock");
        assert!(collected.metadata.aux_blob.is_none());
    }

    #[test]
    fn providers_reject_non_64_byte_report_data_before_hardware_access() {
        let mock = MockTdxQuoteProvider::new(Bytes::from_static(b"fixture"));
        let configfs = ConfigfsTdxQuoteProvider::with_report_dir("/path/that/does/not/exist");

        assert!(matches!(
            mock.quote(&[0u8; 63]),
            Err(TdxRuntimeError::InvalidReportDataLength(63))
        ));
        assert!(matches!(
            configfs.quote(&[0u8; 65]),
            Err(TdxRuntimeError::InvalidReportDataLength(65))
        ));
    }

    #[test]
    fn configfs_provider_reads_quote_and_aux_blob_from_report_dir() {
        let temp = TempDir::new().unwrap();
        let report_dir = temp.path().join("base-tdx-runtime-test");
        fs::create_dir_all(&report_dir).unwrap();
        fs::write(report_dir.join("provider"), TDX_CONFIGFS_PROVIDER_NAME).unwrap();
        fs::write(report_dir.join("outblob"), b"fixture-quote").unwrap();
        fs::write(report_dir.join("auxblob"), b"fixture-aux").unwrap();
        let generation_path = create_generation_fifo(&report_dir);
        let generation_writer = spawn_generation_writer(&generation_path, [7, 8]);

        let provider = ConfigfsTdxQuoteProvider::with_report_dir(&report_dir);
        let collected = provider.quote(&[0x11; TDX_REPORT_DATA_LEN]).unwrap();
        generation_writer.join().unwrap();

        assert_eq!(fs::read(report_dir.join("inblob")).unwrap(), [0x11; TDX_REPORT_DATA_LEN]);
        assert_eq!(collected.quote, Bytes::from_static(b"fixture-quote"));
        assert_eq!(collected.metadata.provider, TDX_CONFIGFS_PROVIDER_NAME);
        assert_eq!(collected.metadata.aux_blob, Some(Bytes::from_static(b"fixture-aux")));
    }

    #[test]
    fn configfs_provider_rejects_generation_counter_mismatch() {
        let temp = TempDir::new().unwrap();
        let report_dir = temp.path().join("base-tdx-runtime-test");
        fs::create_dir_all(&report_dir).unwrap();
        fs::write(report_dir.join("provider"), TDX_CONFIGFS_PROVIDER_NAME).unwrap();
        fs::write(report_dir.join("outblob"), b"fixture-quote").unwrap();
        let generation_path = create_generation_fifo(&report_dir);
        let generation_writer = spawn_generation_writer(&generation_path, [7, 9]);

        let provider = ConfigfsTdxQuoteProvider::with_report_dir(&report_dir);
        assert!(matches!(
            provider.quote(&[0x11; TDX_REPORT_DATA_LEN]),
            Err(TdxRuntimeError::ConfigfsGenerationMismatch { expected: 8, actual: 9 })
        ));
        generation_writer.join().unwrap();
    }

    #[test]
    fn configfs_provider_verifies_generation_counter() {
        let temp = TempDir::new().unwrap();
        let report_dir = temp.path().join("base-tdx-runtime-test");
        fs::create_dir_all(&report_dir).unwrap();
        fs::write(report_dir.join("generation"), "11\n").unwrap();

        let provider = ConfigfsTdxQuoteProvider::with_report_dir(&report_dir);
        provider.verify_generation(11).unwrap();

        fs::write(report_dir.join("generation"), "12\n").unwrap();
        assert!(matches!(
            provider.verify_generation(11),
            Err(TdxRuntimeError::ConfigfsGenerationMismatch { expected: 11, actual: 12 })
        ));
    }

    #[test]
    fn configfs_provider_serializes_access_for_same_report_dir() {
        let temp = TempDir::new().unwrap();
        let report_dir = temp.path().join("base-tdx-runtime-test");

        let first = ConfigfsTdxQuoteProvider::with_report_dir(&report_dir);
        let second = ConfigfsTdxQuoteProvider::with_report_dir(&report_dir);
        let cloned = first.clone();

        assert!(Arc::ptr_eq(&first.quote_lock, &second.quote_lock));
        assert!(Arc::ptr_eq(&first.quote_lock, &cloned.quote_lock));
    }

    #[test]
    fn configfs_provider_rejects_non_tdx_provider_marker() {
        let temp = TempDir::new().unwrap();
        let report_dir = temp.path().join("base-tdx-runtime-test");
        fs::create_dir_all(&report_dir).unwrap();
        fs::write(report_dir.join("provider"), "sev_guest").unwrap();

        let provider = ConfigfsTdxQuoteProvider::with_report_dir(&report_dir);
        assert!(matches!(
            provider.quote(&[0x11; TDX_REPORT_DATA_LEN]),
            Err(TdxRuntimeError::UnexpectedConfigfsProvider(provider)) if provider == "sev_guest"
        ));
    }
}
