//! In-memory store, partitioned by organization, with per-org usage metering.
//!
//! Each org ("tenant") has its own concurrent map, so listing one org's keys never
//! touches another org's data, and orgs don't contend on the same shards.

use std::{
    collections::BinaryHeap,
    sync::{
        atomic::{AtomicI64, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use bytes::Bytes;
use dashmap::DashMap;
use serde::Serialize;

pub const MAX_KEY_LEN: usize = 512;
pub const MAX_ORG_ID_LEN: usize = 128;
pub const MAX_LIST_LIMIT: usize = 1000;

/// Live counters for one organization on this node.
/// `bytes`/`keys` are gauges; `reads`/`writes` only ever go up (since process start).
#[derive(Default)]
struct OrgUsage {
    bytes: AtomicI64,
    keys: AtomicI64,
    reads: AtomicU64,
    writes: AtomicU64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct UsageSnapshot {
    pub org_id: String,
    /// Key + value bytes currently stored.
    pub bytes: i64,
    pub keys: i64,
    /// GET commands served (hits and misses).
    pub reads: u64,
    /// SET / SETEX / PUT / DELETE commands served.
    pub writes: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct KeyInfo {
    pub key: String,
    /// Value size in bytes.
    pub size: usize,
    /// Seconds until expiry, if the key has a TTL.
    pub ttl: Option<u64>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct KeyPage {
    pub keys: Vec<KeyInfo>,
    /// Pass as `after` to get the next page; absent on the last page.
    pub next: Option<String>,
}

struct Entry {
    value: Bytes,
    expires_at: Option<Instant>,
}

impl Entry {
    fn live(&self, now: Instant) -> bool {
        self.expires_at.is_none_or(|t| t > now)
    }
}

#[derive(Default)]
struct Tenant {
    map: DashMap<String, Entry>,
    usage: OrgUsage,
}

impl Tenant {
    fn charge(&self, key: &str, e: &Entry) {
        self.usage
            .bytes
            .fetch_add((key.len() + e.value.len()) as i64, Ordering::Relaxed);
        self.usage.keys.fetch_add(1, Ordering::Relaxed);
    }
    fn uncharge(&self, key: &str, e: &Entry) {
        self.usage
            .bytes
            .fetch_sub((key.len() + e.value.len()) as i64, Ordering::Relaxed);
        self.usage.keys.fetch_sub(1, Ordering::Relaxed);
    }
}

#[derive(Default)]
pub struct Store {
    tenants: DashMap<String, Arc<Tenant>>,
}

/// Keys are shared by both APIs, so no whitespace/control chars (TCP splits on spaces).
pub fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= MAX_KEY_LEN
        && !key.chars().any(|c| c.is_whitespace() || c.is_control())
}

pub fn valid_org_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ORG_ID_LEN
        && !id.chars().any(|c| c.is_whitespace() || c.is_control())
}

impl Store {
    fn tenant(&self, org: &str) -> Arc<Tenant> {
        if let Some(t) = self.tenants.get(org) {
            return t.clone();
        }
        self.tenants.entry(org.to_string()).or_default().clone()
    }

    pub fn get(&self, org: &str, key: &str) -> Option<Bytes> {
        let t = self.tenant(org);
        t.usage.reads.fetch_add(1, Ordering::Relaxed);
        let now = Instant::now();
        {
            // Scope the shard read guard so it's released before remove_if takes a write lock.
            let e = t.map.get(key)?;
            if e.live(now) {
                return Some(e.value.clone());
            }
        }
        if let Some((k, e)) = t.map.remove_if(key, |_, e| !e.live(now)) {
            t.uncharge(&k, &e);
        }
        None
    }

    pub fn set(&self, org: &str, key: &str, value: Bytes, ttl: Option<Duration>) {
        let t = self.tenant(org);
        t.usage.writes.fetch_add(1, Ordering::Relaxed);
        let entry = Entry {
            value,
            expires_at: ttl.map(|d| Instant::now() + d),
        };
        t.charge(key, &entry);
        if let Some(old) = t.map.insert(key.to_string(), entry) {
            t.uncharge(key, &old);
        }
    }

    /// Returns true if a live key was removed.
    pub fn del(&self, org: &str, key: &str) -> bool {
        let t = self.tenant(org);
        t.usage.writes.fetch_add(1, Ordering::Relaxed);
        match t.map.remove(key) {
            Some((k, e)) => {
                t.uncharge(&k, &e);
                e.live(Instant::now())
            }
            None => false,
        }
    }

    /// Live keys in `org` starting with `prefix`, in byte order, after the cursor `after`.
    /// Costs O(n log limit) over the org's keys; meant for dashboards, not hot paths.
    pub fn list(&self, org: &str, prefix: &str, after: Option<&str>, limit: usize) -> KeyPage {
        let limit = limit.clamp(1, MAX_LIST_LIMIT);
        let Some(t) = self.tenants.get(org).map(|t| t.clone()) else {
            return KeyPage {
                keys: vec![],
                next: None,
            };
        };
        let now = Instant::now();
        // Max-heap of the `limit + 1` smallest qualifying keys (+1 tells us if there's more).
        let mut heap: BinaryHeap<(String, usize, Option<u64>)> = BinaryHeap::new();
        for e in t.map.iter() {
            let k = e.key();
            if !k.starts_with(prefix) || after.is_some_and(|a| k.as_str() <= a) || !e.live(now) {
                continue;
            }
            if heap.len() > limit {
                if heap.peek().is_some_and(|(top, ..)| k < top) {
                    heap.pop();
                } else {
                    continue;
                }
            }
            let ttl = e
                .expires_at
                .map(|x| x.saturating_duration_since(now).as_secs());
            heap.push((k.clone(), e.value.len(), ttl));
        }
        let mut items = heap.into_vec();
        items.sort();
        let more = items.len() > limit;
        items.truncate(limit);
        let keys: Vec<KeyInfo> = items
            .into_iter()
            .map(|(key, size, ttl)| KeyInfo { key, size, ttl })
            .collect();
        let next = if more {
            keys.last().map(|k| k.key.clone())
        } else {
            None
        };
        KeyPage { keys, next }
    }

    /// Remove expired entries; called periodically.
    pub fn sweep(&self) -> usize {
        let now = Instant::now();
        let mut removed = 0;
        for t in self.tenants.iter() {
            t.map.retain(|k, e| {
                let keep = e.live(now);
                if !keep {
                    t.uncharge(k, e);
                    removed += 1;
                }
                keep
            });
        }
        removed
    }

    pub fn usage(&self, org: &str) -> UsageSnapshot {
        match self.tenants.get(org) {
            Some(t) => snapshot(org, &t),
            None => UsageSnapshot {
                org_id: org.into(),
                bytes: 0,
                keys: 0,
                reads: 0,
                writes: 0,
            },
        }
    }

    pub fn all_usage(&self) -> Vec<UsageSnapshot> {
        let mut v: Vec<_> = self
            .tenants
            .iter()
            .map(|e| snapshot(e.key(), e.value()))
            .collect();
        v.sort_by(|a, b| a.org_id.cmp(&b.org_id));
        v
    }

    pub fn len(&self) -> usize {
        self.tenants.iter().map(|t| t.map.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn snapshot(org: &str, t: &Tenant) -> UsageSnapshot {
    UsageSnapshot {
        org_id: org.to_string(),
        bytes: t.usage.bytes.load(Ordering::Relaxed),
        keys: t.usage.keys.load(Ordering::Relaxed),
        reads: t.usage.reads.load(Ordering::Relaxed),
        writes: t.usage.writes.load(Ordering::Relaxed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(s: &str) -> Bytes {
        Bytes::copy_from_slice(s.as_bytes())
    }

    #[test]
    fn orgs_are_isolated() {
        let s = Store::default();
        s.set("org-a", "k", b("a"), None);
        s.set("org-b", "k", b("b"), None);
        assert_eq!(s.get("org-a", "k").unwrap(), "a");
        assert_eq!(s.get("org-b", "k").unwrap(), "b");
        assert!(s.del("org-a", "k"));
        assert!(s.get("org-a", "k").is_none());
        assert_eq!(s.get("org-b", "k").unwrap(), "b");
    }

    #[test]
    fn usage_accounting() {
        let s = Store::default();
        s.set("o", "key1", b("12345"), None); // 4 + 5
        s.set("o", "key1", b("123"), None); // overwrite: 4 + 3
        s.set("o", "k2", b("x"), None); // 2 + 1
        let u = s.usage("o");
        assert_eq!((u.bytes, u.keys, u.writes), (10, 2, 3));
        s.get("o", "key1");
        s.get("o", "missing");
        assert!(s.del("o", "k2"));
        let u = s.usage("o");
        assert_eq!((u.bytes, u.keys, u.reads, u.writes), (7, 1, 2, 4));
    }

    #[test]
    fn ttl_expiry_releases_usage() {
        let s = Store::default();
        s.set("o", "a", b("1"), Some(Duration::from_millis(20)));
        s.set("o", "b", b("2"), Some(Duration::from_millis(20)));
        assert_eq!(s.usage("o").keys, 2);
        std::thread::sleep(Duration::from_millis(30));
        assert!(s.get("o", "a").is_none()); // lazy removal
        assert_eq!(s.sweep(), 1); // b
        let u = s.usage("o");
        assert_eq!((u.bytes, u.keys), (0, 0));
        assert!(!s.del("o", "a"));
    }

    #[test]
    fn list_pages_in_order() {
        let s = Store::default();
        for i in (0..25).rev() {
            s.set("o", &format!("user:{i:02}"), b("v"), None);
        }
        s.set("o", "config", b("cfg"), Some(Duration::from_secs(100)));
        s.set("other", "user:99", b("v"), None);

        let p1 = s.list("o", "user:", None, 10);
        assert_eq!(p1.keys.len(), 10);
        assert_eq!(p1.keys[0].key, "user:00");
        assert_eq!(p1.next.as_deref(), Some("user:09"));
        let p2 = s.list("o", "user:", p1.next.as_deref(), 10);
        assert_eq!(p2.keys[0].key, "user:10");
        let p3 = s.list("o", "user:", p2.next.as_deref(), 10);
        assert_eq!(p3.keys.len(), 5);
        assert_eq!(p3.next, None);

        let all = s.list("o", "", None, 100);
        assert_eq!(all.keys.len(), 26);
        assert_eq!(
            all.keys[0],
            KeyInfo {
                key: "config".into(),
                size: 3,
                ttl: Some(99)
            }
        );
        assert!(s.list("nobody", "", None, 10).keys.is_empty());
    }

    #[test]
    fn validation() {
        assert!(valid_key("user:42/profile"));
        assert!(!valid_key(""));
        assert!(!valid_key("has space"));
        assert!(!valid_key(&"x".repeat(MAX_KEY_LEN + 1)));
        assert!(valid_org_id("9b1c3f2e-1111-2222-3333-444455556666"));
        assert!(!valid_org_id("a\0b"));
    }
}
