//! Lightyear metrics Bevy UI (phase 1): minimal Bevy UI to visualize selected metrics
//! using native bevy_ui nodes, similar to diagnostics UI style.
//!
//! This reads metrics from `bevy_metrics_dashboard::registry::MetricsRegistry` and
//! displays a compact overlay. Phase 1 focuses on structure; phase 2 will expand metrics.

extern crate alloc;

use alloc::format;
use alloc::string::{ToString};
use alloc::collections::VecDeque;
use alloc::{vec::Vec, vec};
use bevy_app::prelude::*;
use bevy_color::prelude::*;
use bevy_ecs::prelude::*;
use bevy_ecs::relationship::RelatedSpawnerCommands;
use bevy_ui::prelude::*;
use bevy_reflect::prelude::Reflect;
use bevy_text::prelude::*;
use bevy_utils::prelude::*;

use bevy_platform::collections::HashMap;
use metrics_util::MetricKind;
use tracing::info;
use crate::prelude::{ClearBucketsSystem, MetricsRegistry, RegistryPlugin};

#[derive(Resource, Debug, Reflect)]
#[reflect(Resource, Debug)]
pub struct MetricsPanelSettings {
    pub enabled: bool,
    /// Rolling window length for averages (number of frames/samples)
    pub window_len: usize,
}

impl Default for MetricsPanelSettings {
    fn default() -> Self {
        Self { enabled: true, window_len: 50 }
    }
}

#[derive(Component)]
struct MetricsPanelRoot;

#[derive(Component)]
struct MetricLine { name: &'static str, kind: MetricKind }

/// Specification for a single metric line in the UI.
#[derive(Clone)]
pub struct MetricSpec {
    /// Display label for the line
    pub label: &'static str,
    /// Metrics registry key (supports fuzzy match)
    pub name: &'static str,
    /// Kind of metric
    pub kind: MetricKind,
}

impl MetricSpec {
    const fn new(label: &'static str, name: &'static str, kind: MetricKind) -> Self {
        Self { label, name, kind }
    }
}

/// A group of metrics displayed together under a header.
#[derive(Clone)]
pub struct MetricsGroup {
    pub title: &'static str,
    pub items: Vec<MetricSpec>,
}

impl MetricsGroup {
    pub fn new(title: &'static str, items: Vec<MetricSpec>) -> Self {
        Self { title, items }
    }
}

/// Resource configuring the layout (groups and items) for the metrics panel.
#[derive(Resource, Clone)]
pub struct MetricsPanelLayout {
    pub groups: Vec<MetricsGroup>,
}

impl Default for MetricsPanelLayout {
    fn default() -> Self {
        Self {
            groups: vec![
                MetricsGroup::new(
                    "Frame",
                    vec![MetricSpec::new("Frame Time", "frame.time", MetricKind::Gauge)],
                ),
                MetricsGroup::new(
                    "Replication (ms)",
                    vec![
                        MetricSpec::new("Receive", "replication/receive/time_ms", MetricKind::Gauge),
                        MetricSpec::new("Apply", "replication/apply/time_ms", MetricKind::Gauge),
                        MetricSpec::new("Buffer", "replication/buffer/time_ms", MetricKind::Gauge),
                        MetricSpec::new("Send", "replication/send/time_ms", MetricKind::Gauge),
                    ],
                ),
                // MetricsGroup::new(
                //     "Replication counts",
                //     vec![
                //         MetricSpec::new("Entities (total)", "lightyear.replication.entities.total"),
                //         MetricSpec::new("Entities by name", "lightyear.replication.entities.by_name"),
                //     ],
                // ),
                MetricsGroup::new(
                    "Rollback",
                    vec![
                        MetricSpec::new("Count", "prediction/rollback/count", MetricKind::Counter),
                        MetricSpec::new("Ticks", "prediction/rollback/ticks", MetricKind::Gauge),
                    ],
                ),
                MetricsGroup::new(
                    "Transport",
                    vec![
                        MetricSpec::new("Send packets lost", "transport/packets_lost", MetricKind::Counter),
                        MetricSpec::new("Send (B/s)", "transport/send_bandwidth", MetricKind::Gauge),
                        MetricSpec::new("Recv (B/s)", "transport/recv_bandwidth", MetricKind::Gauge),
                    ],
                ),
            ],
        }
    }
}

pub struct DebugUIPlugin;

impl Plugin for DebugUIPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<RegistryPlugin>() {
            app.add_plugins(RegistryPlugin::default());
        }
        app.register_type::<MetricsPanelSettings>();
        app.init_resource::<MetricsPanelSettings>();
        // Allow users to override the panel layout; provide defaults otherwise
        app.init_resource::<MetricsPanelLayout>();
        app.add_systems(Startup, setup_metrics_panel);
        app.init_resource::<MetricHistory>();
        app.add_systems(Last, (
            update_visibility, sample_metrics_history, update_metrics
        )
            .chain()
            .before(ClearBucketsSystem)
        );
    }
}

fn setup_metrics_panel(
    mut commands: Commands,
    settings: Res<MetricsPanelSettings>,
    layout: Res<MetricsPanelLayout>,
) {
    commands
        .spawn((
            Name::new("Lightyear Metrics"),
            MetricsPanelRoot,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(5.0),
                right: Val::Px(5.0),
                width: Val::Px(300.0),
                padding: UiRect::all(Val::Px(8.0)),
                display: if settings.enabled { Display::Flex } else { Display::None },
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(6.0),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.85)),
            BorderRadius::all(Val::Px(6.0)),
        ))
        .with_children(|cmd| build_metrics_groups(cmd, &layout));
}

fn header(cmd: &mut RelatedSpawnerCommands<ChildOf>, text: &str) {
    cmd.spawn((
        Text::new(text.to_string()),
        TextFont { font_size: 12.0, ..default() },
        TextColor(Color::srgb(0.9, 0.9, 0.9)),
    ));
}

fn line(cmd: &mut RelatedSpawnerCommands<ChildOf>, spec: &MetricSpec) {
    cmd.spawn(Node {
        display: Display::Flex,
        justify_content: JustifyContent::SpaceBetween,
        ..default()
    })
    .with_children(|cmd| {
        cmd.spawn((Text::new(spec.label.to_string()), TextFont { font_size: 10.0, ..default() }));
        cmd.spawn((MetricLine { name: spec.name, kind: spec.kind }, Text::new("-"), TextFont { font_size: 10.0, ..default() }));
    });
}

fn build_metrics_groups(cmd: &mut RelatedSpawnerCommands<ChildOf>, layout: &MetricsPanelLayout) {
    for group in &layout.groups {
        header(cmd, group.title);
        for spec in &group.items {
            line(cmd, spec);
        }
    }
}

fn update_visibility(settings: Res<MetricsPanelSettings>, mut q: Query<&mut Node, With<MetricsPanelRoot>>) {
    if !settings.is_changed() { return; }
    for mut node in &mut q {
        node.display = if settings.enabled { Display::Flex } else { Display::None };
    }
}

fn update_metrics(
    mut q_lines: Query<(&MetricLine, &mut Text)>,
    history: Res<MetricHistory>,
) {
    for (line, mut text) in &mut q_lines {
        if let Some((latest, avg)) = history.latest_and_avg(line.name) {
            text.0 = format!("{:.3} (avg {:.3})", latest, avg);
        } else {
            text.0 = "-".to_string();
        }
    }
}

/// Resource storing rolling windows for metric samples
#[derive(Resource, Default)]
struct MetricHistory {
    windows: HashMap<&'static str, MetricBuffer>,
}

#[derive(Default)]
struct MetricBuffer {
    data: VecDeque<f64>,
    sum: f64,
}

impl MetricBuffer {
    fn push(&mut self, value: f64, cap: usize) {
        self.data.push_back(value);
        self.sum += value;
        while self.data.len() > cap {
            if let Some(v) = self.data.pop_front() {
                self.sum -= v;
            }
        }
    }

    fn latest_and_avg(&self) -> Option<(f64, f64)> {
        let latest = *self.data.back()?;
        let sum = self.sum;
        let avg = if !self.data.is_empty() { sum / self.data.len() as f64 } else { 0.0 };
        Some((latest, avg))
    }

}

impl MetricHistory {
    fn push(&mut self, name: &'static str, value: f64, cap: usize) {
        let buffer = self.windows.entry(name).or_insert_with(|| MetricBuffer::default());
        buffer.push(value, cap);
    }

    fn latest_and_avg(&self, name: &'static str) -> Option<(f64, f64)> {
        let deque = self.windows.get(name)?;
        deque.latest_and_avg()
    }
}

/// Fetch the latest metric value from the MetricRegistry and push it to the history
fn sample_metrics_history(
    registry: Res<MetricsRegistry>,
    settings: Res<MetricsPanelSettings>,
    mut history: ResMut<MetricHistory>,
    q_lines: Query<&MetricLine>,
) {
    let cap = settings.window_len.max(1);
    for line in &q_lines {
        if let Some(sample) = fetch_metric_value(registry.as_ref(), line) {
            history.push(line.name, sample, cap);
        }
    }
}

fn fetch_metric_value(reg: &MetricsRegistry, line: &MetricLine) -> Option<f64> {
    match line.kind {
        MetricKind::Counter => {
            reg.get_counter_value(line.name)
        }
        MetricKind::Gauge => {
            reg.get_gauge_value(line.name)
        }
        MetricKind::Histogram => {
            reg.get_histogram_mean(line.name)
        }
    }
}
