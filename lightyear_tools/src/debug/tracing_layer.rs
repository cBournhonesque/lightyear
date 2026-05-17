//! Tracing subscriber layer for structured `lightyear_debug` events.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use std::eprintln;
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use bevy_app::App;
use bevy_log::{BoxedFmtLayer, BoxedLayer, LogPlugin};
use serde_json::{Map, Number, Value};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::filter::FilterFn;
use tracing_subscriber::layer::Context;

use crate::debug::metadata::current_debug_frame_id;
use crate::debug::schema::{LIGHTYEAR_DEBUG_TARGET, is_lightyear_debug_target};

/// Environment variable used by [`LightyearDebugLayer::from_env`].
pub const LIGHTYEAR_DEBUG_FILE_ENV: &str = "LIGHTYEAR_DEBUG_FILE";

const PROMOTED_FIELDS: &[&str] = &[
    "app_id",
    "action",
    "buffer_len",
    "bytes",
    "category",
    "channel",
    "channel_id",
    "client_id",
    "component",
    "confirmed_tick",
    "direction",
    "entity",
    "end_tick",
    "input_tick",
    "interpolation_tick",
    "jitter_ms",
    "kind",
    "link_entity",
    "local_id",
    "local_tick",
    "message_id",
    "message_name",
    "message_net_id",
    "packet_id",
    "packet_loss",
    "remote_entity",
    "remote_id",
    "remote_peer",
    "remote_tick",
    "role",
    "rollback_tick",
    "rtt_ms",
    "run_id",
    "sample_point",
    "schedule",
    "send_bytes",
    "server_tick",
    "source_entity",
    "system",
    "system_set",
    "tick",
    "tick_id",
    "num_messages",
    "priority",
    "value",
];

/// JSONL formatter layer for events whose target starts with `lightyear_debug`.
pub struct LightyearDebugLayer {
    writer: Mutex<BufWriter<Box<dyn Write + Send + 'static>>>,
}

impl LightyearDebugLayer {
    pub fn stderr() -> Self {
        Self::writer(io::stderr())
    }

    pub fn file(path: impl AsRef<Path>) -> io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self::from_file(file))
    }

    pub fn from_file(file: File) -> Self {
        Self::writer(file)
    }

    /// Build a debug layer from any writer.
    pub fn writer(writer: impl Write + Send + 'static) -> Self {
        Self {
            writer: Mutex::new(BufWriter::new(Box::new(writer))),
        }
    }

    /// Writes to `LIGHTYEAR_DEBUG_FILE` when set, otherwise stderr.
    pub fn from_env() -> io::Result<Self> {
        match std::env::var_os(LIGHTYEAR_DEBUG_FILE_ENV) {
            Some(path) => Self::file(path),
            None => Ok(Self::stderr()),
        }
    }

    fn write_event(&self, event: &Event<'_>) {
        let metadata = event.metadata();
        if !is_lightyear_debug_target(metadata.target()) {
            return;
        }

        let mut visitor = JsonFieldVisitor::default();
        event.record(&mut visitor);

        let mut root = Map::new();
        root.insert("timestamp".to_string(), Value::from(unix_timestamp_ns()));
        root.insert(
            "frame_id".to_string(),
            Value::from(current_debug_frame_id()),
        );
        root.insert("target".to_string(), Value::from(metadata.target()));
        root.insert("level".to_string(), Value::from(metadata.level().as_str()));

        if let Some(category) = category_from_target(metadata.target()) {
            root.insert("category".to_string(), Value::from(category));
        }

        for field in PROMOTED_FIELDS {
            if let Some(value) = visitor.fields.remove(*field) {
                root.insert((*field).to_string(), value);
            }
        }

        root.insert("fields".to_string(), Value::Object(visitor.fields));

        let Ok(mut writer) = self.writer.lock() else {
            return;
        };
        if serde_json::to_writer(&mut *writer, &Value::Object(root)).is_ok() {
            let _ = writer.write_all(b"\n");
        }
    }
}

impl Drop for LightyearDebugLayer {
    fn drop(&mut self) {
        if let Ok(writer) = self.writer.get_mut() {
            let _ = writer.flush();
        }
    }
}

fn unix_timestamp_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos().min(u64::MAX as u128) as u64)
        .unwrap_or_default()
}

impl<S> Layer<S> for LightyearDebugLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        self.write_event(event);
    }
}

/// Bevy `LogPlugin::custom_layer` hook that installs [`LightyearDebugLayer`].
pub fn lightyear_debug_custom_layer(_: &mut App) -> Option<BoxedLayer> {
    match LightyearDebugLayer::from_env() {
        Ok(layer) => Some(Box::new(layer)),
        Err(error) => {
            eprintln!("failed to initialize lightyear debug layer: {error}");
            None
        }
    }
}

/// Bevy `LogPlugin::fmt_layer` hook that keeps Bevy's default formatter away
/// from `lightyear_debug::*` events.
pub fn non_lightyear_debug_fmt_layer(_: &mut App) -> Option<BoxedFmtLayer> {
    Some(Box::new(
        tracing_subscriber::fmt::Layer::default()
            .with_writer(io::stderr)
            .with_filter(FilterFn::new(|metadata| {
                !is_lightyear_debug_target(metadata.target())
            })),
    ))
}

/// Convenience `LogPlugin` with JSONL debug output and filtered regular logs.
pub fn lightyear_debug_log_plugin() -> LogPlugin {
    LogPlugin {
        custom_layer: lightyear_debug_custom_layer,
        fmt_layer: non_lightyear_debug_fmt_layer,
        ..Default::default()
    }
}

fn category_from_target(target: &str) -> Option<&str> {
    target
        .strip_prefix(LIGHTYEAR_DEBUG_TARGET)?
        .strip_prefix("::")?
        .split("::")
        .next()
        .filter(|category| !category.is_empty())
}

#[derive(Default)]
struct JsonFieldVisitor {
    fields: Map<String, Value>,
}

impl JsonFieldVisitor {
    fn insert(&mut self, field: &Field, value: Value) {
        self.fields.insert(field.name().to_string(), value);
    }
}

impl Visit for JsonFieldVisitor {
    fn record_f64(&mut self, field: &Field, value: f64) {
        let value = Number::from_f64(value)
            .map(Value::Number)
            .unwrap_or(Value::Null);
        self.insert(field, value);
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.insert(field, Value::from(value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.insert(field, Value::from(value));
    }

    fn record_i128(&mut self, field: &Field, value: i128) {
        match i64::try_from(value) {
            Ok(value) => self.insert(field, Value::from(value)),
            Err(_) => self.insert(field, Value::from(value.to_string())),
        }
    }

    fn record_u128(&mut self, field: &Field, value: u128) {
        match u64::try_from(value) {
            Ok(value) => self.insert(field, Value::from(value)),
            Err(_) => self.insert(field, Value::from(value.to_string())),
        }
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.insert(field, Value::from(value));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.insert(field, Value::from(value));
    }

    fn record_bytes(&mut self, field: &Field, value: &[u8]) {
        self.insert(field, Value::from(format!("{value:?}")));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn core::fmt::Debug) {
        let formatted = format!("{value:?}");
        let value = JsonDebugValue::decode(&formatted)
            .or_else(|| serde_json::from_str::<Value>(&formatted).ok())
            .unwrap_or(Value::from(formatted));
        self.insert(field, value);
    }
}

pub(crate) struct JsonDebugValue(pub Value);

impl JsonDebugValue {
    const PREFIX: &'static str = "__lightyear_json__:";

    fn decode(formatted: &str) -> Option<Value> {
        formatted
            .strip_prefix(Self::PREFIX)
            .and_then(|json| serde_json::from_str(json).ok())
    }
}

impl core::fmt::Debug for JsonDebugValue {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}{}", Self::PREFIX, self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::sync::Arc;
    use alloc::vec::Vec;
    use std::sync::Mutex as StdMutex;
    use tracing::dispatcher;
    use tracing_subscriber::prelude::*;

    #[derive(Clone, Default)]
    struct SharedWriter(Arc<StdMutex<Vec<u8>>>);

    impl Write for SharedWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn lightyear_layer_writes_promoted_fields_and_filters_other_targets() {
        let writer = SharedWriter::default();
        let output = writer.0.clone();
        let layer = LightyearDebugLayer::writer(writer);
        let subscriber = tracing_subscriber::registry().with(layer);
        let dispatch = dispatcher::Dispatch::new(subscriber);

        dispatcher::with_default(&dispatch, || {
            tracing::trace!(
                target: "lightyear_debug::component",
                kind = "component_value",
                component = "my_crate::Position",
                value = ?JsonDebugValue(serde_json::json!({"x": 1.0, "y": 2.0})),
                message_name = ?"example::Message",
                "component row"
            );
            tracing::trace!(target: "not_lightyear_debug", kind = "ignored");
        });
        drop(dispatch);

        let bytes = output.lock().unwrap().clone();
        let text = String::from_utf8(bytes).unwrap();
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), 1);

        let row: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(row["target"], "lightyear_debug::component");
        assert_eq!(row["category"], "component");
        assert_eq!(row["kind"], "component_value");
        assert_eq!(row["component"], "my_crate::Position");
        assert_eq!(row["message_name"], "example::Message");
        assert_eq!(row["value"], serde_json::json!({"x": 1.0, "y": 2.0}));
        assert!(row.get("timestamp").is_some());
        assert!(row.get("frame_id").is_some());
    }
}
