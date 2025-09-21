//! Lightyear metrics Bevy UI (phase 1): minimal Bevy UI to visualize selected metrics
//! using native bevy_ui nodes, similar to diagnostics UI style.
//!
//! This reads metrics from `bevy_metrics_dashboard::registry::MetricsRegistry` and
//! displays a compact overlay. Phase 1 focuses on structure; phase 2 will expand metrics.

extern crate alloc;

use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::ToString;
use alloc::{vec, vec::Vec};
use bevy_app::prelude::*;
use bevy_color::prelude::*;
use bevy_ecs::prelude::*;
use bevy_ecs::relationship::RelatedSpawnerCommands;
use bevy_reflect::prelude::Reflect;
use bevy_text::prelude::*;
use bevy_time::prelude::*;
use bevy_ui::prelude::*;
use bevy_utils::prelude::*;

use crate::prelude::{ClearBucketsSystem, MetricsRegistry, RegistryPlugin};
use bevy_platform::collections::HashMap;
use metrics::Key;
use metrics_util::{CompositeKey, MetricKind};
#[allow(unused_imports)]
use tracing::info;

#[derive(Resource, Debug, Reflect)]
#[reflect(Resource, Debug)]
pub struct MetricsPanelSettings {
    pub enabled: bool,
    /// Rolling window length for averages (number of frames/samples)
    pub window_len: usize,
    /// Alpha value for the background color
    pub alpha: f32,
}

impl Default for MetricsPanelSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            window_len: 50,
            alpha: 0.2,
        }
    }
}

#[derive(Component)]
struct MetricsPanelRoot;

#[derive(Component)]
struct MetricLine {
    spec: MetricSpec,
}

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetricDirection {
    Send,
    Receive,
    Neutral,
}

#[derive(Component)]
struct ValueText;

#[derive(Component)]
struct DirectionMarker(pub MetricDirection);

/// Specification for a single metric line in the UI.
#[derive(Clone)]
pub struct MetricSpec {
    /// Display label for the line
    pub label: &'static str,
    /// Metrics registry key (supports fuzzy match)
    pub key: CompositeKey,
    /// If True, we need to divide the results per second
    pub per_second: bool,
    /// Direction of the metric
    pub direction: MetricDirection,
}

impl MetricSpec {
    const fn new(label: &'static str, key: CompositeKey) -> Self {
        Self {
            label,
            key,
            per_second: false,
            direction: MetricDirection::Neutral,
        }
    }

    fn with_per_second(mut self, per_second: bool) -> Self {
        self.per_second = per_second;
        self
    }

    fn with_direction(mut self, dir: MetricDirection) -> Self {
        self.direction = dir;
        self
    }
}

/// A subsection of metrics within a section.
#[derive(Clone)]
pub struct MetricsSubsection {
    pub title: &'static str,
    pub items: Vec<MetricSpec>,
}

impl MetricsSubsection {
    pub fn new(title: &'static str, items: Vec<MetricSpec>) -> Self {
        Self { title, items }
    }
}

/// A section that can contain multiple subsections.
#[derive(Clone)]
pub struct MetricsSection {
    pub title: &'static str,
    pub subsections: Vec<MetricsSubsection>,
}

impl MetricsSection {
    pub fn new(title: &'static str, subsections: Vec<MetricsSubsection>) -> Self {
        Self { title, subsections }
    }
}

/// Resource configuring the layout (sections/subsections/items) for the metrics panel.
#[derive(Resource, Clone)]
pub struct MetricsPanelLayout {
    pub sections: Vec<MetricsSection>,
}

impl Default for MetricsPanelLayout {
    fn default() -> Self {
        Self {
            sections: vec![
                MetricsSection::new(
                    "Profiler (ms)",
                    vec![
                        MetricsSubsection::new(
                            "Replication",
                            vec![
                                MetricSpec::new(
                                    "Receive messages",
                                    CompositeKey::new(
                                        MetricKind::Gauge,
                                        Key::from_name("replication/receive/time_ms"),
                                    ),
                                )
                                .with_direction(MetricDirection::Receive),
                                MetricSpec::new(
                                    "Apply to World",
                                    CompositeKey::new(
                                        MetricKind::Gauge,
                                        Key::from_name("replication/apply/time_ms"),
                                    ),
                                )
                                .with_direction(MetricDirection::Receive),
                                MetricSpec::new(
                                    "Buffer messages",
                                    CompositeKey::new(
                                        MetricKind::Gauge,
                                        Key::from_name("replication/buffer/time_ms"),
                                    ),
                                )
                                .with_direction(MetricDirection::Send),
                                MetricSpec::new(
                                    "Send messages",
                                    CompositeKey::new(
                                        MetricKind::Gauge,
                                        Key::from_name("replication/send/time_ms"),
                                    ),
                                )
                                .with_direction(MetricDirection::Send),
                            ],
                        ),
                        MetricsSubsection::new(
                            "Transport",
                            vec![
                                MetricSpec::new(
                                    "Receive",
                                    CompositeKey::new(
                                        MetricKind::Gauge,
                                        Key::from_name("transport/recv/time_ms"),
                                    ),
                                )
                                .with_direction(MetricDirection::Receive),
                                MetricSpec::new(
                                    "Send",
                                    CompositeKey::new(
                                        MetricKind::Gauge,
                                        Key::from_name("transport/send/time_ms"),
                                    ),
                                )
                                .with_direction(MetricDirection::Send),
                            ],
                        ),
                    ],
                ),
                MetricsSection::new(
                    "Prediction",
                    vec![MetricsSubsection::new(
                        "Rollback",
                        vec![
                            MetricSpec::new(
                                "Count",
                                CompositeKey::new(
                                    MetricKind::Counter,
                                    Key::from_name("prediction/rollback/count"),
                                ),
                            ),
                            MetricSpec::new(
                                "Ticks",
                                CompositeKey::new(
                                    MetricKind::Gauge,
                                    Key::from_name("prediction/rollback/ticks"),
                                ),
                            ),
                        ],
                    )],
                ),
                MetricsSection::new(
                    "Bandwidth",
                    vec![
                        MetricsSubsection::new(
                            "Total",
                            vec![
                                MetricSpec::new(
                                    "Send",
                                    CompositeKey::new(
                                        MetricKind::Gauge,
                                        Key::from_name("transport/send_bytes"),
                                    ),
                                )
                                .with_per_second(true)
                                .with_direction(MetricDirection::Send),
                                MetricSpec::new(
                                    "Recv",
                                    CompositeKey::new(
                                        MetricKind::Gauge,
                                        Key::from_name("transport/recv_bytes"),
                                    ),
                                )
                                .with_per_second(true)
                                .with_direction(MetricDirection::Receive),
                                MetricSpec::new(
                                    "Packets lost",
                                    CompositeKey::new(
                                        MetricKind::Counter,
                                        Key::from_name("transport/packets_lost"),
                                    ),
                                )
                                .with_direction(MetricDirection::Send),
                            ],
                        ),
                        MetricsSubsection::new(
                            "Replication",
                            vec![
                                MetricSpec::new(
                                    "Send Actions (bytes/s)",
                                    CompositeKey::new(
                                        MetricKind::Gauge,
                                        Key::from_parts(
                                            "channel/send_bytes",
                                            &[(
                                                "channel",
                                                "lightyear_replication::message::ActionsChannel",
                                            )],
                                        ),
                                    ),
                                )
                                .with_per_second(true)
                                .with_direction(MetricDirection::Send),
                                MetricSpec::new(
                                    "Send Updates (bytes/s)",
                                    CompositeKey::new(
                                        MetricKind::Gauge,
                                        Key::from_parts(
                                            "channel/send_bytes",
                                            &[(
                                                "channel",
                                                "lightyear_replication::message::UpdatesChannel",
                                            )],
                                        ),
                                    ),
                                )
                                .with_per_second(true)
                                .with_direction(MetricDirection::Send),
                                MetricSpec::new(
                                    "Send Actions (num messages)",
                                    CompositeKey::new(
                                        MetricKind::Gauge,
                                        Key::from_parts(
                                            "channel/send_messages",
                                            &[(
                                                "channel",
                                                "lightyear_replication::message::ActionsChannel",
                                            )],
                                        ),
                                    ),
                                )
                                .with_per_second(true)
                                .with_direction(MetricDirection::Send),
                                MetricSpec::new(
                                    "Send Updates (num messages)",
                                    CompositeKey::new(
                                        MetricKind::Gauge,
                                        Key::from_parts(
                                            "channel/send_messages",
                                            &[(
                                                "channel",
                                                "lightyear_replication::message::UpdatesChannel",
                                            )],
                                        ),
                                    ),
                                )
                                .with_per_second(true)
                                .with_direction(MetricDirection::Send),
                                MetricSpec::new(
                                    "Recv Actions (bytes/s)",
                                    CompositeKey::new(
                                        MetricKind::Gauge,
                                        Key::from_parts(
                                            "channel/recv_bytes",
                                            &[(
                                                "channel",
                                                "lightyear_replication::message::ActionsChannel",
                                            )],
                                        ),
                                    ),
                                )
                                .with_per_second(true)
                                .with_direction(MetricDirection::Receive),
                                MetricSpec::new(
                                    "Recv Updates (bytes/s)",
                                    CompositeKey::new(
                                        MetricKind::Gauge,
                                        Key::from_parts(
                                            "channel/recv_bytes",
                                            &[(
                                                "channel",
                                                "lightyear_replication::message::UpdatesChannel",
                                            )],
                                        ),
                                    ),
                                )
                                .with_per_second(true)
                                .with_direction(MetricDirection::Receive),
                                MetricSpec::new(
                                    "Recv Actions (num messages)",
                                    CompositeKey::new(
                                        MetricKind::Gauge,
                                        Key::from_parts(
                                            "channel/recv_messages",
                                            &[(
                                                "channel",
                                                "lightyear_replication::message::ActionsChannel",
                                            )],
                                        ),
                                    ),
                                )
                                .with_per_second(true)
                                .with_direction(MetricDirection::Receive),
                                MetricSpec::new(
                                    "Recv Updates (num messages)",
                                    CompositeKey::new(
                                        MetricKind::Gauge,
                                        Key::from_parts(
                                            "channel/recv_messages",
                                            &[(
                                                "channel",
                                                "lightyear_replication::message::UpdatesChannel",
                                            )],
                                        ),
                                    ),
                                )
                                .with_per_second(true)
                                .with_direction(MetricDirection::Receive),
                            ],
                        ),
                        MetricsSubsection::new(
                            "Inputs",
                            vec![
                                MetricSpec::new(
                                    "Send Inputs (bytes/s)",
                                    CompositeKey::new(
                                        MetricKind::Gauge,
                                        Key::from_parts(
                                            "channel/send_bytes",
                                            &[("channel", "lightyear_inputs::InputChannel")],
                                        ),
                                    ),
                                )
                                .with_per_second(true)
                                .with_direction(MetricDirection::Send),
                                MetricSpec::new(
                                    "Recv Inputs (bytes/s)",
                                    CompositeKey::new(
                                        MetricKind::Gauge,
                                        Key::from_parts(
                                            "channel/recv_bytes",
                                            &[("channel", "lightyear_inputs::InputChannel")],
                                        ),
                                    ),
                                )
                                .with_per_second(true)
                                .with_direction(MetricDirection::Receive),
                                MetricSpec::new(
                                    "Send Inputs (num messages)",
                                    CompositeKey::new(
                                        MetricKind::Gauge,
                                        Key::from_parts(
                                            "channel/send_messages",
                                            &[("channel", "lightyear_inputs::InputChannel")],
                                        ),
                                    ),
                                )
                                .with_per_second(true)
                                .with_direction(MetricDirection::Send),
                                MetricSpec::new(
                                    "Recv Inputs (num messages)",
                                    CompositeKey::new(
                                        MetricKind::Gauge,
                                        Key::from_parts(
                                            "channel/recv_messages",
                                            &[("channel", "lightyear_inputs::InputChannel")],
                                        ),
                                    ),
                                )
                                .with_per_second(true)
                                .with_direction(MetricDirection::Receive),
                            ],
                        ),
                        MetricsSubsection::new(
                            "Sync",
                            vec![
                                MetricSpec::new(
                                    "Send Ping (bytes/s)",
                                    CompositeKey::new(
                                        MetricKind::Gauge,
                                        Key::from_parts(
                                            "channel/send_bytes",
                                            &[("channel", "lightyear_sync::ping::PingChannel")],
                                        ),
                                    ),
                                )
                                .with_per_second(true)
                                .with_direction(MetricDirection::Send),
                                MetricSpec::new(
                                    "Recv Ping (bytes/s)",
                                    CompositeKey::new(
                                        MetricKind::Gauge,
                                        Key::from_parts(
                                            "channel/recv_bytes",
                                            &[("channel", "lightyear_sync::ping::PingChannel")],
                                        ),
                                    ),
                                )
                                .with_per_second(true)
                                .with_direction(MetricDirection::Receive),
                                MetricSpec::new(
                                    "Send Ping (num messages)",
                                    CompositeKey::new(
                                        MetricKind::Gauge,
                                        Key::from_parts(
                                            "channel/send_messages",
                                            &[("channel", "lightyear_sync::ping::PingChannel")],
                                        ),
                                    ),
                                )
                                .with_per_second(true)
                                .with_direction(MetricDirection::Send),
                                MetricSpec::new(
                                    "Recv Ping (num messages)",
                                    CompositeKey::new(
                                        MetricKind::Gauge,
                                        Key::from_parts(
                                            "channel/recv_messages",
                                            &[("channel", "lightyear_sync::ping::PingChannel")],
                                        ),
                                    ),
                                )
                                .with_per_second(true)
                                .with_direction(MetricDirection::Receive),
                            ],
                        ),
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
        app.init_resource::<MetricsPanelLayout>();
        app.insert_resource(VisibilityFilter {
            show_send: true,
            show_recv: true,
        });
        app.init_resource::<CollapseState>();
        app.add_systems(Startup, setup_metrics_panel);
        app.init_resource::<MetricHistory>();
        app.add_systems(
            Last,
            (
                update_visibility,
                handle_button_interactions,
                update_collapsible_displays,
                sample_metrics_history,
                update_metrics,
            )
                .chain()
                .before(ClearBucketsSystem),
        );
    }
}

#[derive(Component)]
struct SectionHeader {
    id: u32,
}

#[derive(Component)]
struct SubsectionHeader {
    id: u32,
}

#[derive(Component)]
struct CollapsibleContent {
    id: u32,
}

#[derive(Resource, Default)]
struct CollapseState {
    sections: HashMap<u32, bool>,
    subsections: HashMap<u32, bool>,
}

#[derive(Resource, Default)]
struct VisibilityFilter {
    show_send: bool,
    show_recv: bool,
}

#[derive(Component)]
struct SendToggle;

#[derive(Component)]
struct RecvToggle;

#[derive(Component, Default)]
struct PerChannelInput;

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
                display: if settings.enabled {
                    Display::Flex
                } else {
                    Display::None
                },
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(6.0),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, settings.alpha)),
            BorderRadius::all(Val::Px(6.0)),
        ))
        .with_children(|cmd| {
            // Top controls row
            cmd.spawn(Node {
                display: Display::Flex,
                justify_content: JustifyContent::SpaceBetween,
                column_gap: Val::Px(8.0),
                ..default()
            })
            .with_children(|cmd| {
                cmd.spawn((
                    SendToggle,
                    Button,
                    Text::new("Send"),
                    TextFont {
                        font_size: 10.0,
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.0, 0.6, 0.0, settings.alpha)),
                ));
                cmd.spawn((
                    RecvToggle,
                    Button,
                    Text::new("Receive"),
                    TextFont {
                        font_size: 10.0,
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.0, 0.6, 0.0, settings.alpha)),
                ));
            });

            build_sections(cmd, &layout, &settings);
        });
}

fn line(
    cmd: &mut RelatedSpawnerCommands<ChildOf>,
    spec: &MetricSpec,
    settings: &MetricsPanelSettings,
) {
    cmd.spawn((
        MetricLine { spec: spec.clone() },
        DirectionMarker(spec.direction),
        Node {
            display: Display::Flex,
            justify_content: JustifyContent::SpaceBetween,
            ..default()
        },
        BackgroundColor(Color::srgba(0.32, 0.32, 0.32, settings.alpha)),
    ))
    .with_children(|cmd| {
        cmd.spawn((
            Text::new(spec.label.to_string()),
            TextFont {
                font_size: 10.0,
                ..default()
            },
        ));
        cmd.spawn((
            ValueText,
            Text::new("-"),
            TextFont {
                font_size: 10.0,
                ..default()
            },
        ));
    });
}

fn build_sections(
    cmd: &mut RelatedSpawnerCommands<ChildOf>,
    layout: &MetricsPanelLayout,
    settings: &MetricsPanelSettings,
) {
    let mut section_id: u32 = 1;
    let mut subsection_id: u32 = 1000;
    for section in &layout.sections {
        // Section header button
        cmd.spawn((
            SectionHeader { id: section_id },
            Button,
            Node {
                display: Display::Flex,
                justify_content: JustifyContent::SpaceBetween,
                ..default()
            },
        ))
        .with_children(|cmd| {
            cmd.spawn((
                Text::new(format!("> {}", section.title)),
                TextFont {
                    font_size: 12.0,
                    ..default()
                },
                TextColor(Color::srgba(0.9, 0.9, 0.9, settings.alpha)),
            ));
        });
        // Section content container
        cmd.spawn((
            CollapsibleContent { id: section_id },
            Node {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(4.0),
                ..default()
            },
            BackgroundColor(Color::srgba(0.12, 0.12, 0.12, settings.alpha)),
        ))
        .with_children(|cmd| {
            for subsection in &section.subsections {
                // Subsection header
                cmd.spawn((
                    SubsectionHeader { id: subsection_id },
                    Button,
                    Node {
                        display: Display::Flex,
                        justify_content: JustifyContent::SpaceBetween,
                        ..default()
                    },
                ))
                .with_children(|cmd| {
                    cmd.spawn((
                        Text::new(format!("> {}", subsection.title)),
                        TextFont {
                            font_size: 11.0,
                            ..default()
                        },
                    ));
                });
                // Subsection content
                cmd.spawn((
                    CollapsibleContent { id: subsection_id },
                    Node {
                        display: Display::Flex,
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(2.0),
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.24, 0.24, 0.24, settings.alpha)),
                ))
                .with_children(|cmd| {
                    for spec in &subsection.items {
                        line(cmd, spec, settings);
                    }
                });
                subsection_id += 1;
            }
        });
        section_id += 1;
    }
}

fn handle_button_interactions(
    mut collapse: ResMut<CollapseState>,
    mut vis: ResMut<VisibilityFilter>,
    settings: Res<MetricsPanelSettings>,
    mut uis: ParamSet<(
        Query<(&SectionHeader, &mut Interaction, &mut Children)>,
        Query<(&SubsectionHeader, &mut Interaction, &mut Children)>,
        Query<(&mut Interaction, &mut BackgroundColor), With<SendToggle>>,
        Query<(&mut Interaction, &mut BackgroundColor), With<RecvToggle>>,
        Query<&mut Text>,
    )>,
) {
    // Section toggles
    {
        let section_events: Vec<(u32, Option<Entity>)> = {
            let mut out = Vec::new();
            for (header, interaction, children) in uis.p0().iter() {
                if *interaction == Interaction::Pressed {
                    out.push((header.id, children.first().copied()));
                }
            }
            out
        };
        for (id, first_child) in section_events {
            let entry = collapse.sections.entry(id).or_insert(false);
            *entry = !*entry;
            if let Some(child) = first_child
                && let Ok(mut text) = uis.p4().get_mut(child)
            {
                let label = text
                    .0
                    .trim_start_matches('V')
                    .trim_start_matches('>')
                    .trim();
                text.0 = if *entry {
                    format!("> {}", label)
                } else {
                    format!("V {}", label)
                };
            }
        }
        // reset interactions
        for (_h, mut interaction, _c) in &mut uis.p0() {
            if *interaction == Interaction::Pressed {
                *interaction = Interaction::None;
            }
        }
    }
    // Subsection toggles
    {
        let subsection_events: Vec<(u32, Option<Entity>)> = {
            let mut out = Vec::new();
            for (header, interaction, children) in uis.p1().iter() {
                if *interaction == Interaction::Pressed {
                    out.push((header.id, children.first().copied()));
                }
            }
            out
        };
        for (id, first_child) in subsection_events {
            let entry = collapse.subsections.entry(id).or_insert(false);
            *entry = !*entry;
            if let Some(child) = first_child
                && let Ok(mut text) = uis.p4().get_mut(child)
            {
                let label = text
                    .0
                    .trim_start_matches('V')
                    .trim_start_matches('>')
                    .trim();
                text.0 = if *entry {
                    format!("> {}", label)
                } else {
                    format!("V {}", label)
                };
            }
        }
        for (_h, mut interaction, _c) in &mut uis.p1() {
            if *interaction == Interaction::Pressed {
                *interaction = Interaction::None;
            }
        }
    }
    // Send/Recv toggles
    if let Ok((mut inter, mut bg)) = uis.p2().single_mut()
        && *inter == Interaction::Pressed
    {
        vis.show_send = !vis.show_send;
        bg.0 = if vis.show_send {
            Color::srgba(0.0, 0.6, 0.0, settings.alpha)
        } else {
            Color::srgba(0.2, 0.2, 0.2, settings.alpha)
        };
        *inter = Interaction::None;
    }
    if let Ok((mut inter, mut bg)) = uis.p3().single_mut()
        && *inter == Interaction::Pressed
    {
        vis.show_recv = !vis.show_recv;
        bg.0 = if vis.show_recv {
            Color::srgba(0.0, 0.6, 0.0, settings.alpha)
        } else {
            Color::srgba(0.2, 0.2, 0.2, settings.alpha)
        };
        *inter = Interaction::None;
    }
}

fn update_collapsible_displays(
    collapse: Res<CollapseState>,
    mut q_cont: Query<(&CollapsibleContent, &mut Node)>,
) {
    if !collapse.is_changed() {
        return;
    }
    for (c, mut node) in &mut q_cont {
        let collapsed = collapse.sections.get(&c.id).copied().unwrap_or(false)
            || collapse.subsections.get(&c.id).copied().unwrap_or(false);
        node.display = if collapsed {
            Display::None
        } else {
            Display::Flex
        };
    }
}

fn update_visibility(
    settings: Res<MetricsPanelSettings>,
    mut q: Query<&mut Node, With<MetricsPanelRoot>>,
) {
    if !settings.is_changed() {
        return;
    }
    for mut node in &mut q {
        node.display = if settings.enabled {
            Display::Flex
        } else {
            Display::None
        };
    }
}

// Update the metric text on the UI
fn update_metrics(
    time: Res<Time<Real>>,
    vis: Res<VisibilityFilter>,
    mut q_lines: Query<(&MetricLine, &DirectionMarker, &mut Node, &Children)>,
    mut q_values: Query<&mut Text, With<ValueText>>,
    history: Res<MetricHistory>,
) {
    // Section/subsection collapsing is handled by containers; here we only apply Send/Recv filtering
    for (line, dir, mut node, children) in &mut q_lines {
        // Filter by Send/Receive toggles
        let show = match dir.0 {
            MetricDirection::Send => vis.show_send,
            MetricDirection::Receive => vis.show_recv,
            MetricDirection::Neutral => true,
        };
        node.display = if show { Display::Flex } else { Display::None };

        // Update value text
        if show
            && let Some(buffer) = history.buffers.get(&line.spec.key.key().get_hash())
            && let Some(latest) = buffer.latest()
        {
            let avg = buffer.avg().unwrap();
            if let Some(&child) = children.get(1)
                && let Ok(mut text) = q_values.get_mut(child)
            {
                text.0 = format!("{:.3} (avg {:.3})", latest, avg);
            }
        }
    }
}

/// Resource storing rolling windows for metric samples
#[derive(Resource, Default)]
struct MetricHistory {
    // the key is the hash of the metric key
    buffers: HashMap<u64, MetricBuffer>,
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

    fn latest(&self) -> Option<f64> {
        self.data.back().copied()
    }

    fn avg(&self) -> Option<f64> {
        let sum = self.sum;
        if self.data.is_empty() {
            None
        } else {
            Some(sum / self.data.len() as f64)
        }
    }
}

/// Fetch the latest metric value from the MetricRegistry and push it to the history
fn sample_metrics_history(
    time: Res<Time<Real>>,
    registry: Res<MetricsRegistry>,
    settings: Res<MetricsPanelSettings>,
    mut history: ResMut<MetricHistory>,
    q_lines: Query<&MetricLine>,
) {
    let delta = time.delta().as_secs_f64();
    let cap = settings.window_len.max(1);
    for line in &q_lines {
        if let Some(mut sample) = fetch_metric_value(registry.as_ref(), line) {
            let key = line.spec.key.key().get_hash();
            let buffer = history.buffers.entry(key).or_default();
            if line.spec.per_second {
                sample /= delta;
            };
            buffer.push(sample, cap);
            // TODO: we might want to also do this for non-per-second metrics!
            // if the metric is per second, we need to reset the value of the metric.
            // The reason is that the metric could be incremented multiple times inside a single frame
            // (for example if we receive multiple messages in the same channel of the same frame)
            if line.spec.per_second {
                registry.reset_metric(&line.spec.key);
            }
        }
    }
}

fn fetch_metric_value(reg: &MetricsRegistry, line: &MetricLine) -> Option<f64> {
    match line.spec.key.kind() {
        MetricKind::Counter => reg.get_counter_value(line.spec.key.key()),
        MetricKind::Gauge => reg.get_gauge_value(line.spec.key.key()),
        MetricKind::Histogram => reg.get_histogram_mean(line.spec.key.key()),
    }
}
