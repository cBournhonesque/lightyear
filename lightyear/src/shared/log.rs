#![allow(clippy::type_complexity)]
//! Log plugin that also potentially emits metrics to Prometheus.
//! This cannot be used in conjunction with Bevy's `LogPlugin`
use bevy::app::App;
use bevy::log::BoxedLayer;
#[cfg(feature = "metrics")]
use metrics_tracing_context::{MetricsLayer, TracingContextLayer};

pub fn add_log_layer(app: &mut App) -> Option<BoxedLayer> {
    // add metrics_tracing_context support
    #[cfg(feature = "metrics")]
    {
        let metrics_layer = MetricsLayer::new();
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
        Some(metrics_layer.boxed())
    }
    #[cfg(not(feature = "metrics"))]
    {
        None
    }
}
