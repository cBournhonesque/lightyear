//! Lightyear metrics Bevy UI (phase 1): minimal Bevy UI to visualize selected metrics
//! using native bevy_ui nodes, similar to diagnostics UI style.
//!
//! This reads metrics from `bevy_metrics_dashboard::registry::MetricsRegistry` and
//! displays a compact overlay. Phase 1 focuses on structure; phase 2 will expand metrics.

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
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
use bevy_metrics_dashboard::registry::{MetricsRegistry, SearchResult};
use bevy_metrics_dashboard::metrics_util::{MetricKind, storage::AtomicBucket};
use core::sync::atomic::Ordering;

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
struct MetricLine { name: &'static str }

/// Specification for a single metric line in the UI.
#[derive(Clone)]
pub struct MetricSpec {
    /// Display label for the line
    pub label: &'static str,
    /// Metrics registry key (supports fuzzy match)
    pub name: &'static str,
}

impl MetricSpec {
    pub const fn new(label: &'static str, name: &'static str) -> Self {
        Self { label, name }
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
                    vec![MetricSpec::new("Frame Time", "frame.time")],
                ),
                MetricsGroup::new(
                    "Timing (ms)",
                    vec![
                        MetricSpec::new("Receive", "lightyear.receive.time"),
                        MetricSpec::new("Send", "lightyear.send.time"),
                        MetricSpec::new("Prediction", "lightyear.prediction.time"),
                        MetricSpec::new("Interpolation", "lightyear.interpolation.time"),
                    ],
                ),
                MetricsGroup::new(
                    "Replication counts",
                    vec![
                        MetricSpec::new("Entities (total)", "lightyear.replication.entities.total"),
                        MetricSpec::new("Entities by name", "lightyear.replication.entities.by_name"),
                    ],
                ),
                MetricsGroup::new(
                    "Rollback",
                    vec![
                        MetricSpec::new("Count", "lightyear.rollback.count"),
                        MetricSpec::new("Depth", "lightyear.rollback.depth"),
                    ],
                ),
                MetricsGroup::new(
                    "Bandwidth (bytes/s)",
                    vec![
                        MetricSpec::new("Send total", "lightyear.bandwidth.send.total"),
                        MetricSpec::new("Recv total", "lightyear.bandwidth.recv.total"),
                    ],
                ),
            ],
        }
    }
}

pub struct DebugUIPlugin;

impl Plugin for DebugUIPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<bevy_metrics_dashboard::RegistryPlugin>() {
            app.add_plugins(bevy_metrics_dashboard::RegistryPlugin::default());
        }
        app.register_type::<MetricsPanelSettings>();
        app.init_resource::<MetricsPanelSettings>();
        // Allow users to override the panel layout; provide defaults otherwise
        app.init_resource::<MetricsPanelLayout>();
        app.add_systems(Startup, setup_metrics_panel);
        app.init_resource::<MetricHistory>();
        app.add_systems(Update, (update_visibility, sample_metrics_history, update_metrics).chain());
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

fn line(cmd: &mut RelatedSpawnerCommands<ChildOf>, label: &str, name: &'static str) {
    cmd.spawn(Node {
        display: Display::Flex,
        justify_content: JustifyContent::SpaceBetween,
        ..default()
    })
    .with_children(|cmd| {
        cmd.spawn((Text::new(label.to_string()), TextFont { font_size: 10.0, ..default() }));
        cmd.spawn((MetricLine { name }, Text::new("-"), TextFont { font_size: 10.0, ..default() }));
    });
}

fn build_metrics_groups(cmd: &mut RelatedSpawnerCommands<ChildOf>, layout: &MetricsPanelLayout) {
    for group in &layout.groups {
        header(cmd, group.title);
        for spec in &group.items {
            line(cmd, spec.label, spec.name);
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
    registry: Option<Res<MetricsRegistry>>, // provided by bevy_metrics_dashboard::RegistryPlugin
    mut q_lines: Query<(&MetricLine, &mut Text)>,
    history: Res<MetricHistory>,
) {
    let Some(registry) = registry else { return; };
    for (line, mut text) in &mut q_lines {
        if let Some((latest, avg)) = history.latest_and_avg(line.name) {
            text.0 = format!("{:.3} (avg {:.3})", latest, avg);
        } else {
            let value = render_first_metric(registry.as_ref(), line.name);
            text.0 = value;
        }
    }
}

fn render_first_metric(reg: &MetricsRegistry, name: &str) -> String {
    let results = reg.fuzzy_search_by_name(name);
    if results.is_empty() { return "(no data)".to_string(); }

    // Prefer histogram mean; otherwise show gauge/counter label only for now
    for SearchResult { key, .. } in &results {
        match key.kind {
            MetricKind::Histogram => {
                let hist = reg.get_or_create_histogram(&key.key);
                let mean = mean_histogram(&hist);
                return format!("{:.3}", mean);
            }
            MetricKind::Gauge => {
                return "gauge".to_string();
            }
            MetricKind::Counter => {
                return "counter".to_string();
            }
        }
    }
    "-".to_string()
}

fn mean_histogram(bucket: &AtomicBucket<f64>) -> f64 {
    let mut total = 0.0f64;
    let mut count = 0;
    bucket.data_with(|block| {
        block.iter().for_each(|v| {
            total += *v;
            count += 1;
        });
    });
    if count > 0 { total / count as f64 } else { 0.0 }
}

/// Resource storing rolling windows for metric samples
#[derive(Resource, Default)]
struct MetricHistory {
    windows: HashMap<&'static str, VecDeque<f64>>, // keyed by metric name string (from MetricLine)
}

impl MetricHistory {
    fn push(&mut self, name: &'static str, value: f64, cap: usize) {
        let deque = self.windows.entry(name).or_insert_with(VecDeque::new);
        deque.push_back(value);
        while deque.len() > cap {
            deque.pop_front();
        }
    }

    fn latest_and_avg(&self, name: &'static str) -> Option<(f64, f64)> {
        let deque = self.windows.get(name)?;
        let latest = *deque.back()?;
        let sum: f64 = deque.iter().copied().sum();
        let avg = if !deque.is_empty() { sum / deque.len() as f64 } else { latest };
        Some((latest, avg))
    }
}

fn sample_metrics_history(
    registry: Option<Res<MetricsRegistry>>,
    settings: Res<MetricsPanelSettings>,
    mut history: ResMut<MetricHistory>,
    q_lines: Query<&MetricLine>,
) {
    let Some(registry) = registry else { return; };
    let cap = settings.window_len.max(1);
    for line in &q_lines {
        if let Some(sample) = sample_metric_once(registry.as_ref(), line.name) {
            history.push(line.name, sample, cap);
        }
    }
}

fn sample_metric_once(reg: &MetricsRegistry, name: &str) -> Option<f64> {
    let results = reg.fuzzy_search_by_name(name);
    if results.is_empty() { return None; }
    // Prefer histogram mean, otherwise gauge; counters ignored for now
    for SearchResult { key, .. } in &results {
        match key.kind {
            MetricKind::Histogram => {
                let hist = reg.get_or_create_histogram(&key.key);
                let mean = mean_histogram(&hist);
                return Some(mean);
            }
            MetricKind::Gauge => {
                let g = reg.get_or_create_gauge(&key.key);
                let v = g.load(Ordering::Relaxed) as f64;
                return Some(v);
            }
            MetricKind::Counter => {}
        }
    }
    None
}
