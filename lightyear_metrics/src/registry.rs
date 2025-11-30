//! The process-global metrics registry.

use alloc::vec::Vec;
use bevy_ecs::prelude::{Res, Resource};
use bevy_platform::sync::{Arc, atomic::Ordering};
use metrics::atomics::AtomicU64;
use metrics::{Counter, Gauge, Histogram, Key, KeyName, Metadata, Recorder, SharedString, Unit};
use metrics_util::{
    CompositeKey, MetricKind,
    registry::{AtomicStorage, Registry},
    storage::AtomicBucket,
};
#[cfg(feature = "std")]
use {metrics::set_global_recorder, std::sync::LazyLock};

/// In some cases it can be convenient to use a global registry instead of the [`MetricsRegistry`] resource
#[cfg(feature = "std")]
pub static GLOBAL_RECORDER: LazyLock<MetricsRegistry> = LazyLock::new(|| {
    let registry = MetricsRegistry::default();
    _ = set_global_recorder(registry.clone());
    registry.clone()
});

/// Tracks all metrics in the current process.
///
/// You may never need to interact with this, unless you want to call
/// [`set_global_recorder`](metrics::set_global_recorder) manually and provide a
/// clone of that same registry to the [`RegistryPlugin`](super::plugin::MetricsPlugin).
#[derive(Clone, Resource)]
pub struct MetricsRegistry {
    inner: Arc<Inner>,
}

impl MetricsRegistry {
    pub fn fetch_metric_value(&self, key: &CompositeKey) -> Option<f64> {
        match key.kind() {
            MetricKind::Counter => self.get_counter_value(key.key()),
            MetricKind::Gauge => self.get_gauge_value(key.key()),
            MetricKind::Histogram => self.get_histogram_mean(key.key()),
        }
    }

    #[cfg(feature = "std")]
    pub fn fetch_global_metric_value(key: &CompositeKey) -> Option<f64> {
        GLOBAL_RECORDER.fetch_metric_value(key)
    }
}

struct Inner {
    registry: Registry<Key, AtomicStorage>,
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

    pub fn reset_metric(&self, key: &CompositeKey) -> Option<()> {
        match key.kind() {
            MetricKind::Gauge => {
                let gauge = self.inner.registry.get_gauge(key.key())?;
                gauge.store(0, Ordering::Relaxed);
            }
            MetricKind::Counter => {
                let counter = self.inner.registry.get_counter(key.key())?;
                counter.store(0, Ordering::Relaxed);
            }
            MetricKind::Histogram => {
                let histogram = self.inner.registry.get_histogram(key.key())?;
                histogram.clear();
            }
        };
        Some(())
    }

    /// Get the value of a Counter identified by a string
    pub fn get_counter_value(&self, key: &Key) -> Option<f64> {
        let counter = self.inner.registry.get_counter(key)?;
        Some(counter.load(Ordering::Relaxed) as f64)
    }

    pub fn get_gauge_value(&self, key: &Key) -> Option<f64> {
        let counter = self.inner.registry.get_gauge(key)?;
        Some(f64::from_bits(counter.load(Ordering::Relaxed)))
    }

    pub fn get_histogram_mean(&self, key: &Key) -> Option<f64> {
        let bucket = self.inner.registry.get_histogram(key)?;
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
    pub fn get_or_create_counter(&self, key: &Key) -> Arc<AtomicU64> {
        self.inner.registry.get_or_create_counter(key, Arc::clone)
    }
    #[allow(missing_docs)]
    pub fn get_or_create_gauge(&self, key: &Key) -> Arc<AtomicU64> {
        self.inner.registry.get_or_create_gauge(key, Arc::clone)
    }
    #[allow(missing_docs)]
    pub fn get_or_create_histogram(&self, key: &Key) -> Arc<AtomicBucket<f64>> {
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
            results.push(make_search_result(MetricKind::Histogram, key));
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

fn make_search_result(kind: MetricKind, key: &Key) -> SearchResult {
    let key = CompositeKey::new(kind, key.clone());
    SearchResult { key }
}

impl Default for MetricsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Metadata for a metric.
#[allow(missing_docs)]
#[derive(Clone)]
pub struct SearchResult {
    pub key: CompositeKey,
}

impl Recorder for MetricsRegistry {
    fn describe_counter(&self, key_name: KeyName, unit: Option<Unit>, description: SharedString) {}

    fn describe_gauge(&self, key_name: KeyName, unit: Option<Unit>, description: SharedString) {}

    fn describe_histogram(&self, key_name: KeyName, unit: Option<Unit>, description: SharedString) {
    }

    fn register_counter(&self, key: &Key, _metadata: &Metadata<'_>) -> Counter {
        self.inner
            .registry
            .get_or_create_counter(key, |c| c.clone().into())
    }

    fn register_gauge(&self, key: &Key, _metadata: &Metadata<'_>) -> Gauge {
        self.inner
            .registry
            .get_or_create_gauge(key, |c| c.clone().into())
    }

    fn register_histogram(&self, key: &Key, _metadata: &Metadata<'_>) -> Histogram {
        self.inner
            .registry
            .get_or_create_histogram(key, |c| c.clone().into())
    }
}
