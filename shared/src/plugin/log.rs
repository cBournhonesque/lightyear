#![allow(clippy::type_complexity)]
#![warn(missing_docs)]
//! This crate provides logging functions and configuration for [Bevy](https://bevyengine.org)
//! apps, and automatically configures platform specific log handlers (i.e. WASM or Android).

#[cfg(feature = "trace")]
use std::panic;
use std::thread;

#[cfg(target_os = "android")]
mod android_tracing;

#[cfg(feature = "trace_tracy_memory")]
#[global_allocator]
static GLOBAL: tracy_client::ProfiledAllocator<std::alloc::System> =
    tracy_client::ProfiledAllocator::new(std::alloc::System, 100);

use bevy::prelude::{App, Plugin};
use tracing::Level;
// use tracing_log::LogTracer;

#[cfg(feature = "metrics")]
use metrics_tracing_context::{MetricsLayer, TracingContextLayer};
#[cfg(feature = "metrics")]
use metrics_util::layers::{Layer, Stack};

#[cfg(feature = "tracing-chrome")]
use tracing_subscriber::fmt::{format::DefaultFields, FormattedFields};
use tracing_subscriber::{prelude::*, registry::Registry, EnvFilter};

/// Adds logging to Apps.
///
/// # Panics
///
/// This plugin should not be added multiple times in the same process. This plugin
/// sets up global logging configuration for **all** Apps in a given process, and
/// rerunning the same initialization multiple times will lead to a panic.
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
            filter: "wgpu=error,naga=warn".to_string(),
            level: Level::INFO,
        }
    }
}

impl Plugin for LogPlugin {
    #[cfg_attr(not(feature = "tracing-chrome"), allow(unused_variables))]
    fn build(&self, app: &mut App) {
        #[cfg(feature = "trace")]
        {
            let old_handler = panic::take_hook();
            panic::set_hook(Box::new(move |infos| {
                println!("{}", tracing_error::SpanTrace::capture());
                old_handler(infos);
            }));
        }

        let finished_subscriber;
        let default_filter = { format!("{},{}", self.level, self.filter) };
        let filter_layer = EnvFilter::try_from_default_env()
            .or_else(|_| EnvFilter::try_new(&default_filter))
            .unwrap();
        let subscriber = Registry::default().with(filter_layer);

        #[cfg(feature = "trace")]
        let subscriber = subscriber.with(tracing_error::ErrorLayer::default());

        #[cfg(all(not(target_arch = "wasm32"), not(target_os = "android")))]
        {
            #[cfg(feature = "tracing-chrome")]
            let chrome_layer = {
                let mut layer = tracing_chrome::ChromeLayerBuilder::new();
                if let Ok(path) = std::env::var("TRACE_CHROME") {
                    layer = layer.file(path);
                }
                let (chrome_layer, guard) = layer
                    .name_fn(Box::new(|event_or_span| match event_or_span {
                        tracing_chrome::EventOrSpan::Event(event) => event.metadata().name().into(),
                        tracing_chrome::EventOrSpan::Span(span) => {
                            if let Some(fields) =
                                span.extensions().get::<FormattedFields<DefaultFields>>()
                            {
                                format!("{}: {}", span.metadata().name(), fields.fields.as_str())
                            } else {
                                span.metadata().name().into()
                            }
                        }
                    }))
                    .build();
                app.world.insert_non_send_resource(guard);
                chrome_layer
            };

            #[cfg(feature = "tracing-tracy")]
            let tracy_layer = tracing_tracy::TracyLayer::new();

            let fmt_layer = tracing_subscriber::fmt::Layer::default()
                // log span enters
                .with_span_events(tracing_subscriber::fmt::format::FmtSpan::ENTER)
                // .with_max_level(tracing::Level::TRACE)
                .with_writer(std::io::stderr);

            // bevy_render::renderer logs a `tracy.frame_mark` event every frame
            // at Level::INFO. Formatted logs should omit it.
            #[cfg(feature = "tracing-tracy")]
            let fmt_layer =
                fmt_layer.with_filter(tracing_subscriber::filter::FilterFn::new(|meta| {
                    meta.fields().field("tracy.frame_mark").is_none()
                }));

            let subscriber = subscriber.with(fmt_layer);

            #[cfg(feature = "tracing-chrome")]
            let subscriber = subscriber.with(chrome_layer);
            #[cfg(feature = "tracing-tracy")]
            let subscriber = subscriber.with(tracy_layer);

            // // add metrics_tracing_context support
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
                    thread::Builder::new()
                        .spawn(move || runtime.block_on(exporter))
                        .unwrap();
                    metrics::set_boxed_recorder(Box::new(traced_recorder));
                } else {
                }
            }

            finished_subscriber = subscriber;
        }

        #[cfg(target_arch = "wasm32")]
        {
            console_error_panic_hook::set_once();
            finished_subscriber = subscriber.with(tracing_wasm::WASMLayer::new(
                tracing_wasm::WASMLayerConfig::default(),
            ));
        }

        #[cfg(target_os = "android")]
        {
            finished_subscriber = subscriber.with(android_tracing::AndroidLayer::default());
        }

        // let logger_already_set = LogTracer::init().is_err();
        // let subscriber_already_set =
        tracing::subscriber::set_global_default(finished_subscriber).is_err();
        //
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
