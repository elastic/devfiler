// Copyright Elasticsearch B.V. and/or licensed to Elasticsearch B.V. under one
// or more contributor license agreements. See the NOTICE file distributed with
// this work for additional information regarding copyright
// ownership. Elasticsearch B.V. licenses this file to you under
// the Apache License, Version 2.0 (the "License"); you may
// not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing,
// software distributed under the License is distributed on an
// "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied.  See the License for the
// specific language governing permissions and limitations
// under the License.

//! Caching and background computation.

use arc_swap::ArcSwap;
use chrono::{DateTime, Duration, Utc};
use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;
use std::ops::Deref;
use std::sync::atomic::Ordering::SeqCst;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::Arc;

/// Global counter for updates of [`Cached`] instances.
static UPDATE_CTR: AtomicUsize = AtomicUsize::new(1);

struct CachedValue<V> {
    created: DateTime<Utc>,
    input_hash: u64,
    value: V,
}

struct CachedInner<V> {
    max_lifetime: Duration,
    being_constructed: AtomicBool,
    value: ArcSwap<CachedValue<V>>,
}

/// Cached, background computed value.
///
/// Created via [`Cached::default`].
pub struct Cached<V: Default>(Arc<CachedInner<V>>);

impl<V: Default> Default for Cached<V> {
    fn default() -> Self {
        Self(Arc::new(CachedInner {
            max_lifetime: Duration::try_seconds(1).unwrap(),
            being_constructed: AtomicBool::new(false),
            value: ArcSwap::new(Arc::new(CachedValue {
                created: DateTime::UNIX_EPOCH,
                input_hash: 0,
                value: V::default(),
            })),
        }))
    }
}

impl<V: Default> Cached<V> {
    /// Get the cached value or initiate computing it in the background.
    ///
    /// The value passed as `key` is hashed and used to determine whether the
    /// cached value needs to be recomputed. Everything that influences the
    /// construction of the cached value should be included here.
    ///
    /// This function always returns immediately. If the `key` changes, the
    /// cache will be refreshed in a background task using the user-provided
    /// `create` closure. The cache will continue to return the outdated cached
    /// value until the background task completes.
    ///
    /// On first call, the cache will always return a default constructed [`V`].
    pub fn get_or_create<I, F>(&self, key: I, create: F) -> CachedValueRef<V>
    where
        I: std::hash::Hash,
        F: FnOnce() -> V,
        F: Send + 'static,
        V: Send + Sync + 'static,
    {
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        let new_input_hash = hasher.finish();

        let value = self.0.value.load_full();

        // Check whether existing value is still good.
        let age = Utc::now() - value.created;
        if self.0.max_lifetime >= age && value.input_hash == new_input_hash {
            return CachedValueRef(value);
        }

        // Existing value no longer good: elect a task to update it.
        if let Err(_) = self
            .0
            .being_constructed
            .compare_exchange(false, true, SeqCst, SeqCst)
        {
            // Another task is already on it.
            return CachedValueRef(value);
        }

        // We elected ourselves as the one responsible for construction.
        let this = Arc::clone(&self.0);
        tokio::task::spawn_blocking(move || {
            let new_value = create();
            this.value.store(Arc::new(CachedValue {
                created: Utc::now(),
                input_hash: new_input_hash,
                value: new_value,
            }));
            this.being_constructed.store(false, SeqCst);
            UPDATE_CTR.fetch_add(1, SeqCst);
        });

        CachedValueRef(value)
    }
}

/// Access to the cached value.
pub struct CachedValueRef<V>(Arc<CachedValue<V>>);

impl<V> Deref for CachedValueRef<V> {
    type Target = V;

    fn deref(&self) -> &Self::Target {
        &self.0.value
    }
}

/// Watch for update of **any** [`Cached`] instance.
#[derive(Debug, Default)]
pub struct UpdateWatcher {
    prev_ctr: usize,
}

impl UpdateWatcher {
    /// Check whether any `Cached` instance was updated since last call.
    pub fn any_caches(&mut self) -> bool {
        let new_ctr = UPDATE_CTR.load(SeqCst);
        let old_ctr = std::mem::replace(&mut self.prev_ctr, new_ctr);
        new_ctr != old_ctr
    }
}
