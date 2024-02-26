#![allow(clippy::type_complexity)]
//! Log plugin that also potentially emits metrics to Prometheus.
//! This cannot be used in conjunction with Bevy's `LogPlugin`
use bevy::log::BoxedSubscriber;
use bevy::prelude::Plugin;
#[cfg(feature = "metrics")]
use metrics_tracing_context::{MetricsLayer, TracingContextLayer};
use tracing_subscriber::prelude::*;

pub fn add_log_layer(subscriber: BoxedSubscriber) -> BoxedSubscriber {
    // let fmt_layer = tracing_subscriber::fmt::Layer::default()
    //     // log span enters
    //     .with_span_events(tracing_subscriber::fmt::format::FmtSpan::ENTER)
    //     // .with_max_level(self.level)
    //     .with_writer(std::io::stderr);

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
    // let new_subscriber = tracing_subscriber::Layer::with_subscriber(fmt_layer, subscriber);
    // Box::new(new_subscriber)
    Box::new(subscriber)
}
