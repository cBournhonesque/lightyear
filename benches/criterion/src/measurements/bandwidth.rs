use criterion::Throughput;
use criterion::measurement::{Measurement, ValueFormatter};
use lightyear::metrics::metrics::Key;
use lightyear::metrics::metrics_util::{CompositeKey, MetricKind};
use lightyear::prelude::MetricsRegistry;

pub struct Bandwidth;

pub enum BandwidthChannel {
    Total,
    Replication,
}

impl Bandwidth {
    pub fn value(
        registry: &MetricsRegistry,
        send: bool,
        recv: bool,
        channel: BandwidthChannel,
    ) -> f64 {
        let mut total = 0.0;
        match channel {
            BandwidthChannel::Total => {
                if send {
                    let send_bytes = registry
                        .fetch_metric_value(&CompositeKey::new(
                            MetricKind::Gauge,
                            Key::from_name("transport/send_bytes"),
                        ))
                        .unwrap_or_default();
                    total += send_bytes;
                }
                if recv {
                    let recv_bytes = registry
                        .fetch_metric_value(&CompositeKey::new(
                            MetricKind::Gauge,
                            Key::from_name("transport/recv_bytes"),
                        ))
                        .unwrap_or_default();
                    total += recv_bytes;
                }
            }
            BandwidthChannel::Replication => {
                if send {
                    let send_updates = registry
                        .fetch_metric_value(&CompositeKey::new(
                            MetricKind::Gauge,
                            Key::from_parts(
                                "channel/send_bytes",
                                &[("channel", "lightyear_replication::message::UpdatesChannel")],
                            ),
                        ))
                        .unwrap_or_default();
                    let send_actions = registry
                        .fetch_metric_value(&CompositeKey::new(
                            MetricKind::Gauge,
                            Key::from_parts(
                                "channel/send_bytes",
                                &[("channel", "lightyear_replication::message::ActionsChannel")],
                            ),
                        ))
                        .unwrap_or_default();
                    total += send_updates + send_actions;
                }
                if recv {
                    let recv_updates = registry
                        .fetch_metric_value(&CompositeKey::new(
                            MetricKind::Gauge,
                            Key::from_parts(
                                "channel/recv_bytes",
                                &[("channel", "lightyear_replication::message::UpdatesChannel")],
                            ),
                        ))
                        .unwrap_or_default();
                    let recv_actions = registry
                        .fetch_metric_value(&CompositeKey::new(
                            MetricKind::Gauge,
                            Key::from_parts(
                                "channel/recv_bytes",
                                &[("channel", "lightyear_replication::message::ActionsChannel")],
                            ),
                        ))
                        .unwrap_or_default();
                    total += recv_actions + recv_updates;
                }
            }
        }
        total
    }
}

impl Measurement for Bandwidth {
    type Intermediate = f64;
    type Value = f64;

    fn start(&self) -> Self::Intermediate {
        0.0
    }
    fn end(&self, i: Self::Intermediate) -> Self::Value {
        unimplemented!("unimplement because we use iter_custom")
    }
    fn add(&self, v1: &Self::Value, v2: &Self::Value) -> Self::Value {
        *v1 + *v2
    }
    fn zero(&self) -> Self::Value {
        0.0
    }
    fn to_f64(&self, val: &Self::Value) -> f64 {
        *val
    }
    fn formatter(&self) -> &dyn ValueFormatter {
        &BandwidthFormatter
    }
}

struct BandwidthFormatter;
impl ValueFormatter for BandwidthFormatter {
    fn format_value(&self, value: f64) -> String {
        // The value will be in nanoseconds so we have to convert to half-seconds.
        format!("{:.2} KB", value / 1024.0)
    }

    fn format_throughput(&self, throughput: &Throughput, value: f64) -> String {
        match *throughput {
            Throughput::Bytes(bytes) => format!("{:.2} KB/s", bytes as f64 / (value * 1024.0)),
            Throughput::BytesDecimal(bytes) => {
                format!("{:.2} KB/s", bytes as f64 / (value * 1024.0))
            }
            Throughput::Elements(elems) => format!("{:.2} elem/s", elems as f64 / (value * 1024.0)),
            _ => {
                unimplemented!()
            }
        }
    }

    fn scale_values(&self, ns: f64, values: &mut [f64]) -> &'static str {
        for val in values {
            *val /= 1024.0;
        }
        "KB"
    }

    fn scale_throughputs(
        &self,
        _typical: f64,
        throughput: &Throughput,
        values: &mut [f64],
    ) -> &'static str {
        match *throughput {
            Throughput::Bytes(bytes) => {
                for val in values {
                    *val = (bytes as f64) / (*val * 1024.0)
                }

                "KB/s"
            }
            Throughput::BytesDecimal(bytes) => {
                // Convert nanoseconds/iteration to bytes/half-second.
                for val in values {
                    *val = (bytes as f64) / (*val * 1024.0)
                }
                "KB/s"
            }
            Throughput::Elements(elems) => {
                for val in values {
                    *val = (elems as f64) / (*val * 1024.0)
                }
                "elem/s"
            }
            _ => {
                unimplemented!()
            }
        }
    }

    fn scale_for_machines(&self, values: &mut [f64]) -> &'static str {
        "B"
    }
}
