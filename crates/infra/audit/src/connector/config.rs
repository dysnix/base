use std::time::Duration;

use url::Url;

/// Configuration for the audit connector.
///
/// The connector reads `BundleEvent`s from an mpsc channel, buffers them, and
/// flushes a batch via `base_persistBundleEventBatch` whenever the buffer
/// reaches `max_batch_size`. There is no periodic timer flush — flushes happen
/// on capacity or on shutdown only.
#[derive(Debug, Clone)]
pub struct AuditConnectorConfig {
    /// Audit-archiver RPC endpoint URL.
    pub audit_url: Url,
    /// Maximum events per RPC request before forcing a flush.
    pub max_batch_size: usize,
    /// Per-request timeout for the HTTP client.
    pub request_timeout: Duration,
    /// Maximum RPC send retries before dropping a batch.
    pub max_retries: u32,
    /// Base delay between retries (doubles each attempt).
    pub retry_backoff: Duration,
    /// Maximum time to wait for the connector to drain its buffer on shutdown.
    pub shutdown_timeout: Duration,
}

impl AuditConnectorConfig {
    /// Creates a new config with the given audit URL and default tuning.
    pub const fn new(audit_url: Url) -> Self {
        Self {
            audit_url,
            max_batch_size: 100,
            request_timeout: Duration::from_millis(1000),
            max_retries: 3,
            retry_backoff: Duration::from_millis(100),
            shutdown_timeout: Duration::from_secs(30),
        }
    }

    /// Sets the maximum batch size per request.
    pub const fn with_max_batch_size(mut self, size: usize) -> Self {
        self.max_batch_size = size;
        self
    }

    /// Sets the per-request HTTP timeout.
    pub const fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Sets the max retries.
    pub const fn with_max_retries(mut self, retries: u32) -> Self {
        self.max_retries = retries;
        self
    }

    /// Sets the retry backoff base.
    pub const fn with_retry_backoff(mut self, backoff: Duration) -> Self {
        self.retry_backoff = backoff;
        self
    }

    /// Sets the shutdown drain timeout.
    pub const fn with_shutdown_timeout(mut self, timeout: Duration) -> Self {
        self.shutdown_timeout = timeout;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url() -> Url {
        "http://audit:8545".parse().unwrap()
    }

    #[test]
    fn defaults() {
        let config = AuditConnectorConfig::new(url());
        assert_eq!(config.audit_url, url());
        assert_eq!(config.max_batch_size, 100);
        assert_eq!(config.request_timeout, Duration::from_millis(1000));
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.retry_backoff, Duration::from_millis(100));
        assert_eq!(config.shutdown_timeout, Duration::from_secs(30));
    }

    #[test]
    fn builder_methods() {
        let config = AuditConnectorConfig::new(url())
            .with_max_batch_size(50)
            .with_request_timeout(Duration::from_millis(500))
            .with_max_retries(5)
            .with_retry_backoff(Duration::from_millis(250))
            .with_shutdown_timeout(Duration::from_secs(10));

        assert_eq!(config.max_batch_size, 50);
        assert_eq!(config.request_timeout, Duration::from_millis(500));
        assert_eq!(config.max_retries, 5);
        assert_eq!(config.retry_backoff, Duration::from_millis(250));
        assert_eq!(config.shutdown_timeout, Duration::from_secs(10));
    }
}
