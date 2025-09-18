use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use metrics::set_global_recorder;
use tracing::error;
use crate::metrics::registry::MetricsRegistry;

/// Installs and garbage collects a [`MetricsRegistry`].
///
/// This plugin runs a garbage collector every frame in the
/// [`ClearBucketsSystem`]. Direct consumers of atomic buckets (e.g. plots)
/// should read the documentation of [`ClearBucketsSystem`].
#[derive(Default)]
pub struct RegistryPlugin {
    registry: Option<MetricsRegistry>,
}

/// The [`SystemSet`] from which atomic buckets are cleared.
///
/// Histogram consumer systems should run in the [`Last`] schedule **before**
/// this set to avoid missing samples.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq, SystemSet)]
pub struct ClearBucketsSystem;

impl RegistryPlugin {
    /// Create a default plugin.
    pub fn new() -> Self {
        Self::default()
    }

    /// Provide your own instance of a registry.
    ///
    /// WARNING: Using this constructor will silence errors if it fails to
    /// call [`set_global_recorder`], assuming that the user already did this
    /// manually with `registry`.
    pub fn with_registry(registry: MetricsRegistry) -> Self {
        Self {
            registry: Some(registry),
        }
    }
}

impl Plugin for RegistryPlugin {
    fn build(&self, app: &mut App) {
        let registry = if let Some(registry) = &self.registry {
            _ = set_global_recorder(registry.clone());
            registry.clone()
        } else {
            let registry = MetricsRegistry::default();
            if let Err(e) = set_global_recorder(registry.clone()) {
                error!("Failed to set global recorder: {e}");
            }
            registry
        };
        app.insert_resource(registry).add_systems(
            Last,
            MetricsRegistry::clear_atomic_buckets_system.in_set(ClearBucketsSystem),
        );
    }
}
