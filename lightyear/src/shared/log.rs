#![allow(clippy::type_complexity)]
//! Log plugin that also potentially emits metrics to Prometheus.
//! This cannot be used in conjunction with Bevy's `LogPlugin`
use bevy::prelude::{App, Plugin};
#[cfg(feature = "metrics")]
use metrics_tracing_context::{MetricsLayer, TracingContextLayer};
#[cfg(feature = "metrics")]
use metrics_util::layers::Layer;

use tracing::{warn, Level};
use tracing_subscriber::{prelude::*, registry::Registry, EnvFilter};

use tracing_log::LogTracer;

/// Adds logging to Apps.
///
/// # Panics
///
/// This plugin should not be added multiple times in the same process. This plugin
/// sets up global logging configuration for **all** Apps in a given process, and
/// rerunning the same initialization multiple times will lead to a panic.
// TODO: take directly log config?
pub struct LogPlugin {
    /// Filters logs using the [`EnvFilter`] format
    pub filter: String,

    /// Filters out logs that are "less than" the given level.
    /// This can be further filtered using the `filter` setting.
    pub level: Level,
}

impl Default for LogPlugin {
    fn default() -> Self {
        Self {
            filter: "wgpu=error,wgpu_hal=error,naga=warn,bevy_app=info".to_string(),
            level: Level::INFO,
        }
    }
}

#[derive(Clone)]
/// Configuration to setup logging/metrics
pub struct LogConfig {
    /// Filters logs using the [`EnvFilter`] format
    pub filter: String,

    /// Filters out logs that are "less than" the given level.
    /// This can be further filtered using the `filter` setting.
    pub level: Level,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            filter: "wgpu=error,wgpu_hal=error,naga=warn,bevy_app=info".to_string(),
            level: Level::INFO,
        }
    }
}

impl Plugin for LogPlugin {
    fn build(&self, app: &mut App) {
        let finished_subscriber;
        let default_filter = { format!("{},{}", self.level, self.filter) };
        dbg!(&default_filter);
        let filter_layer = EnvFilter::try_from_default_env()
            .or_else(|_| EnvFilter::try_new(&default_filter))
            .unwrap();
        let subscriber = Registry::default().with(filter_layer);

        let fmt_layer = tracing_subscriber::fmt::Layer::default()
            // log span enters
            .with_span_events(tracing_subscriber::fmt::format::FmtSpan::ENTER)
            // .with_max_level(self.level)
            .with_writer(std::io::stderr);

        // bevy_render::renderer logs a `tracy.frame_mark` event every frame
        // at Level::INFO. Formatted logs should omit it.
        let subscriber = subscriber.with(fmt_layer);

        // add metrics_tracing_context support
        cfg_if::cfg_if! {
            if #[cfg(feature = "metrics")] {
                let subscriber = subscriber.with(MetricsLayer::new());
                // create a prometheus exporter with tracing context support
                let builder = metrics_exporter_prometheus::PrometheusBuilder::new();
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let (recorder, exporter) = {
                    let _g = runtime.enter();
                    builder.build().unwrap()
                };
                // add extra metrics layers
                // Stack::new(recorder)
                // .push(TracingContextLayer::all())
                // .install();
                // runtime.spawn(exporter);

                // Add in tracing
                let traced_recorder = TracingContextLayer::all().layer(recorder);
                std::thread::Builder::new()
                    .spawn(move || runtime.block_on(exporter))
                    .unwrap();
                metrics::set_boxed_recorder(Box::new(traced_recorder));
            } else {
            }
        }

        finished_subscriber = subscriber;

        // let logger_already_set = LogTracer::init().is_err();
        let subscriber_already_set =
            tracing::subscriber::set_global_default(finished_subscriber).is_err();

        // match (logger_already_set, subscriber_already_set) {
        //     (true, true) => warn!(
        //         "Could not set global logger and tracing subscriber as they are already set. Consider disabling LogPlugin."
        //     ),
        //     (true, _) => warn!("Could not set global logger as it is already set. Consider disabling LogPlugin."),
        //     (_, true) => warn!("Could not set global tracing subscriber as it is already set. Consider disabling LogPlugin."),
        //     _ => (),
        // }
    }
}
