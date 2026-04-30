//! In-memory cooldown tracker used for per-IP and per-address faucet limits.

use std::{
    collections::HashMap,
    hash::Hash,
    sync::Mutex,
    time::{Duration, Instant},
};

/// Sweep expired entries once the limiter exceeds this many remembered keys.
const SWEEP_THRESHOLD: usize = 10_000;

/// Tracks the last time a given key was served and answers "is this key in
/// cooldown right now?" queries.
///
/// State is process-local and resets on restart. That is acceptable for
/// vibenet, which wipes chain state on every redeploy anyway.
#[derive(Debug)]
pub struct Limiter<K: Eq + Hash> {
    inner: Mutex<HashMap<K, Instant>>,
}

impl<K: Eq + Hash + Clone> Limiter<K> {
    /// Create an empty limiter.
    pub fn new() -> Self {
        Self { inner: Mutex::new(HashMap::new()) }
    }

    /// If `key` has not been served within `cooldown`, mark it served now and
    /// return `None`. Otherwise return the remaining duration before the next
    /// allowed request.
    pub fn try_acquire(&self, key: K, cooldown: Duration) -> Option<Duration> {
        let mut map = match self.inner.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let now = Instant::now();
        if map.len() > SWEEP_THRESHOLD {
            map.retain(|_, last| now.duration_since(*last) < cooldown);
        }

        if let Some(last) = map.get(&key) {
            let elapsed = now.duration_since(*last);
            if elapsed < cooldown {
                return Some(cooldown - elapsed);
            }
        }

        map.insert(key, now);
        None
    }

    /// Reserve a cooldown slot and release it automatically unless the caller
    /// commits the permit after the protected action succeeds.
    pub fn try_reserve(
        &self,
        key: K,
        cooldown: Duration,
    ) -> Result<LimiterPermit<'_, K>, Duration> {
        self.try_acquire(key.clone(), cooldown)
            .map_or_else(|| Ok(LimiterPermit { limiter: self, key: Some(key) }), Err)
    }

    /// Undo a previous `try_acquire` for `key`. Used when the downstream
    /// action (e.g. sending a transaction) fails and we don't want to punish
    /// the user for our failure.
    pub fn release(&self, key: &K) {
        let mut map = match self.inner.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        map.remove(key);
    }
}

/// A successful limiter reservation that releases itself on drop unless
/// explicitly committed.
#[derive(Debug)]
pub struct LimiterPermit<'a, K: Eq + Hash + Clone> {
    limiter: &'a Limiter<K>,
    key: Option<K>,
}

impl<K: Eq + Hash + Clone> LimiterPermit<'_, K> {
    /// Keep the cooldown entry after the protected action succeeds.
    pub fn commit(mut self) {
        self.key = None;
    }
}

impl<K: Eq + Hash + Clone> Drop for LimiterPermit<'_, K> {
    fn drop(&mut self) {
        if let Some(key) = self.key.take() {
            self.limiter.release(&key);
        }
    }
}

impl<K: Eq + Hash + Clone> Default for Limiter<K> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cooldown_applies_after_first_request() {
        let limiter = Limiter::<&'static str>::new();
        assert!(limiter.try_acquire("a", Duration::from_secs(60)).is_none());
        assert!(limiter.try_acquire("a", Duration::from_secs(60)).is_some());
    }

    #[test]
    fn release_clears_cooldown() {
        let limiter = Limiter::<&'static str>::new();
        assert!(limiter.try_acquire("a", Duration::from_secs(60)).is_none());
        limiter.release(&"a");
        assert!(limiter.try_acquire("a", Duration::from_secs(60)).is_none());
    }

    #[test]
    fn dropped_permit_releases_cooldown() {
        let limiter = Limiter::<&'static str>::new();
        let permit = limiter.try_reserve("a", Duration::from_secs(60)).unwrap();
        assert!(limiter.try_acquire("a", Duration::from_secs(60)).is_some());
        drop(permit);
        assert!(limiter.try_acquire("a", Duration::from_secs(60)).is_none());
    }

    #[test]
    fn committed_permit_keeps_cooldown() {
        let limiter = Limiter::<&'static str>::new();
        let permit = limiter.try_reserve("a", Duration::from_secs(60)).unwrap();
        permit.commit();
        assert!(limiter.try_acquire("a", Duration::from_secs(60)).is_some());
    }
}
