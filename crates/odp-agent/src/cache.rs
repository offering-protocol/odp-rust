use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
    time::{Duration, SystemTime},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CacheRecord {
    pub body: Vec<u8>,
    pub etag: Option<String>,
    pub expires_at: SystemTime,
    pub final_url: String,
    pub last_modified: Option<String>,
    pub status: u16,
    pub stored_at: SystemTime,
}

pub trait Cache: Send + Sync {
    fn delete(&self, key: &str) -> Result<(), String>;
    fn get(&self, key: &str) -> Result<Option<CacheRecord>, String>;
    fn set(&self, key: String, record: CacheRecord) -> Result<(), String>;
}

const DEFAULT_CAPACITY: usize = 256;

/// An in-memory cache bounded by entry count, so a long-lived Agent cannot grow without limit.
/// When it is full the least recently stored record makes room for the new one.
pub struct MemoryCache {
    capacity: usize,
    records: RwLock<BTreeMap<String, CacheRecord>>,
}

impl Default for MemoryCache {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }
}

impl MemoryCache {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A cache holding at most `capacity` records. A capacity of zero keeps nothing.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity,
            records: RwLock::new(BTreeMap::new()),
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.records
            .read()
            .map(|records| records.len())
            .unwrap_or_default()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Cache for MemoryCache {
    fn delete(&self, key: &str) -> Result<(), String> {
        self.records
            .write()
            .map_err(|_| "memory cache lock is poisoned".to_owned())?
            .remove(key);
        Ok(())
    }

    fn get(&self, key: &str) -> Result<Option<CacheRecord>, String> {
        Ok(self
            .records
            .read()
            .map_err(|_| "memory cache lock is poisoned".to_owned())?
            .get(key)
            .cloned())
    }

    fn set(&self, key: String, record: CacheRecord) -> Result<(), String> {
        let mut records = self
            .records
            .write()
            .map_err(|_| "memory cache lock is poisoned".to_owned())?;
        if self.capacity == 0 {
            return Ok(());
        }
        if !records.contains_key(&key) && records.len() >= self.capacity {
            // Capacity is at least one here, so evicting just enough always leaves room.
            let mut by_age = records
                .iter()
                .map(|(name, value)| (value.stored_at, name.clone()))
                .collect::<Vec<_>>();
            by_age.sort();
            for (_, name) in by_age.into_iter().take(records.len() + 1 - self.capacity) {
                records.remove(&name);
            }
        }
        records.insert(key, record);
        Ok(())
    }
}

/// CCH-02: the freshness an Agent assumes when a response supplies none.
///
/// CCH-03 makes each resource class independently configurable, so every class the draft names has
/// its own field rather than borrowing a neighbour's.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CacheFallbacks {
    /// Filter and Sort Definitions.
    pub capabilities: Duration,
    pub collection: Duration,
    pub offering: Duration,
    /// Attribute Schema documents.
    pub schema: Duration,
    /// Search responses, which describe one request and are not reused for the next.
    pub search: Duration,
    pub service_document: Duration,
}

impl Default for CacheFallbacks {
    fn default() -> Self {
        Self {
            capabilities: Duration::from_secs(60 * 60),
            collection: Duration::from_secs(60 * 60),
            offering: Duration::from_secs(5 * 60),
            schema: Duration::from_secs(24 * 60 * 60),
            search: Duration::ZERO,
            service_document: Duration::from_secs(4 * 60 * 60),
        }
    }
}

pub(crate) fn default_cache() -> Arc<dyn Cache> {
    Arc::new(MemoryCache::new())
}
