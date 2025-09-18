//! The process-global metrics registry.

use alloc::{vec::Vec};
use bevy_ecs::prelude::{Res, Resource};
use bevy_platform::{sync::{atomic::Ordering, Arc}};
use metrics::atomics::AtomicU64;
use metrics::{Counter, Gauge, Histogram, Key, KeyName, Metadata, Recorder, SharedString, Unit};
use metrics_util::{
    registry::{AtomicStorage, Registry},
    storage::AtomicBucket,
    MetricKind,
};

/// Tracks all metrics in the current process.
///
/// You may never need to interact with this, unless you want to call
/// [`set_global_recorder`](metrics::set_global_recorder) manually and provide a
/// clone of that same registry to the [`RegistryPlugin`](crate::RegistryPlugin).
#[derive(Clone, Resource)]
pub struct MetricsRegistry {
    inner: Arc<Inner>,
}

struct Inner {
    registry: Registry<metrics::Key, AtomicStorage>,
}

impl Inner {
    fn new() -> Self {
        Self {
            registry: Registry::atomic(),
        }
    }
}

impl MetricsRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner::new()),
        }
    }

    /// Get the value of a Counter identified by a string
    pub fn get_counter_value(&self, name: impl Into<KeyName>) -> Option<f64> {
        let counter = self.inner.registry.get_counter(&Key::from_name(name))?;
        Some(counter.load(Ordering::Relaxed) as f64)
    }

    pub fn get_gauge_value(&self, name: impl Into<KeyName>) -> Option<f64> {
        let counter = self.inner.registry.get_gauge(&Key::from_name(name))?;
        Some(f64::from_bits(counter.load(Ordering::Relaxed)))
    }

    pub fn get_histogram_mean(&self, name: impl Into<KeyName>) -> Option<f64> {
        let bucket = self.inner.registry.get_histogram(&Key::from_name(name))?;
        let mut total = 0.0f64;
        let mut count = 0;
        bucket.data_with(|block| {
            block.iter().for_each(|v| {
                total += *v;
                count += 1;
            });
        });
        Some(if count > 0 { total / count as f64 } else { 0.0 })
    }

    #[allow(missing_docs)]
    pub fn get_or_create_counter(&self, key: &metrics::Key) -> Arc<AtomicU64> {
        self.inner.registry.get_or_create_counter(key, Arc::clone)
    }
    #[allow(missing_docs)]
    pub fn get_or_create_gauge(&self, key: &metrics::Key) -> Arc<AtomicU64> {
        self.inner.registry.get_or_create_gauge(key, Arc::clone)
    }
    #[allow(missing_docs)]
    pub fn get_or_create_histogram(&self, key: &metrics::Key) -> Arc<AtomicBucket<f64>> {
        self.inner.registry.get_or_create_histogram(key, Arc::clone)
    }

    /// Get a search result for every registered metric.
    pub fn all_metrics(&self) -> Vec<SearchResult> {
        let mut results = Vec::new();
        let reg = &self.inner.registry;
        reg.visit_counters(|key, _| {
            results.push(make_search_result(MetricKind::Counter, key));
        });
        reg.visit_gauges(|key, _| {
            results.push(make_search_result(MetricKind::Gauge, key));
        });
        reg.visit_histograms(|key, _| {
            results.push(make_search_result(
                MetricKind::Histogram,
                key,
            ));
        });
        results
    }

    /// Clear all atomic buckets used for storing histogram data.
    pub fn clear_atomic_buckets(&self) {
        self.inner.registry.visit_histograms(|_, h| {
            h.clear();
        });
    }

    pub(crate) fn clear_atomic_buckets_system(registry: Res<Self>) {
        registry.clear_atomic_buckets();
    }
}

fn make_search_result(
    kind: MetricKind,
    key: &metrics::Key,
) -> SearchResult {
    let key = MetricKey::new(key.clone(), kind);
    SearchResult { key }
}

impl Default for MetricsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Identifies some metric in the registry.
#[allow(missing_docs)]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct MetricKey {
    pub key: metrics::Key,
    pub kind: MetricKind,
}

impl MetricKey {
    #[allow(missing_docs)]
    pub fn new(key: metrics::Key, kind: MetricKind) -> Self {
        Self { key, kind }
    }
}

/// Metadata for a metric.
#[allow(missing_docs)]
#[derive(Clone)]
pub struct SearchResult {
    pub key: MetricKey,
}


impl Recorder for MetricsRegistry {
    fn describe_counter(&self, key_name: KeyName, unit: Option<Unit>, description: SharedString) {
    }

    fn describe_gauge(&self, key_name: KeyName, unit: Option<Unit>, description: SharedString) {
    }

    fn describe_histogram(&self, key_name: KeyName, unit: Option<Unit>, description: SharedString) {
    }

    fn register_counter(&self, key: &metrics::Key, _metadata: &Metadata<'_>) -> Counter {
        self.inner
            .registry
            .get_or_create_counter(key, |c| c.clone().into())
    }

    fn register_gauge(&self, key: &metrics::Key, _metadata: &Metadata<'_>) -> Gauge {
        self.inner
            .registry
            .get_or_create_gauge(key, |c| c.clone().into())
    }

    fn register_histogram(&self, key: &metrics::Key, _metadata: &Metadata<'_>) -> Histogram {
        self.inner
            .registry
            .get_or_create_histogram(key, |c| c.clone().into())
    }
}
