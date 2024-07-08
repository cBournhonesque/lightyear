use avian2d::prelude::{Position, Rotation};
/// Utility plugin to display a text label next to an entity.
///
/// Label will track parent position, ignoring rotation.
use bevy::prelude::*;
use lightyear::{
    client::{
        interpolation::{plugin::InterpolationSet, VisualInterpolateStatus},
        prediction::plugin::PredictionSet,
    },
    prelude::client::Correction,
};

pub struct EntityLabelPlugin;

impl Plugin for EntityLabelPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (label_added, label_changed));

        app.add_systems(
            PostUpdate,
            fix_entity_label_rotations.before(bevy::transform::systems::propagate_transforms),
        );
    }
}

#[derive(Component)]
pub struct EntityLabel {
    pub text: String,
    pub sub_text: String,
    pub offset: Vec2,
    pub inherit_rotation: bool,
    pub z: f32,
    pub size: f32,
    pub color: Color,
}

impl Default for EntityLabel {
    fn default() -> Self {
        Self {
            text: "".to_owned(),
            sub_text: "".to_owned(),
            offset: Vec2::ZERO,
            inherit_rotation: false,
            z: 10.0,
            size: 13.0,
            color: bevy::color::palettes::css::ANTIQUE_WHITE.into(),
        }
    }
}

/// Marker for labels that are children (with TextBundles) of entities with EntityLabel
#[derive(Component)]
pub struct EntityLabelChild;

/// Add the child entity containing the Text2dBundle
fn label_added(
    asset_server: Res<AssetServer>,
    q: Query<(Entity, &EntityLabel), Added<EntityLabel>>,
    mut commands: Commands,
) {
    let font: Handle<Font> = Default::default();
    let mut ts = TextStyle {
        font: font.clone(),
        font_size: 16.0,
        color: Color::WHITE,
    };
    let mut ts_sub = TextStyle {
        font,
        font_size: 13.0,
        color: Color::WHITE,
    };
    for (e, label) in q.iter() {
        ts.font_size = label.size;
        ts_sub.font_size = label.size * 0.85;
        ts.color = label.color;
        ts_sub.color = label.color.with_alpha(0.6);
        commands
            .spawn((
                EntityLabelChild,
                Text2dBundle {
                    text: Text::from_sections([
                        TextSection::new(label.text.clone(), ts.clone()),
                        TextSection::new("\n", ts.clone()),
                        TextSection::new(label.sub_text.clone(), ts_sub.clone()),
                    ])
                    .with_no_wrap()
                    .with_justify(JustifyText::Center),
                    transform: Transform::from_translation(Vec3::new(
                        label.offset.x,
                        label.offset.y,
                        label.z,
                    )),
                    ..default()
                },
            ))
            .set_parent(e);
    }
}

/// modify text when EntityLabel changes
fn label_changed(
    q_parents: Query<(&EntityLabel, &Children), Changed<EntityLabel>>,
    mut q_children: Query<
        (&mut Text, &mut Transform),
        (With<EntityLabelChild>, Without<EntityLabel>),
    >,
) {
    for (label, children) in q_parents.iter() {
        for child in children.iter() {
            if let Ok((mut text, mut transform)) = q_children.get_mut(*child) {
                assert_eq!(text.sections.len(), 3);

                if label.text != text.sections[0].value {
                    text.sections[0].value.clone_from(&label.text);
                }
                text.sections[0].style.font_size = label.size;
                text.sections[0].style.color = label.color;

                if label.sub_text != text.sections[2].value {
                    text.sections[2].value.clone_from(&label.sub_text);
                }
                text.sections[2].style.font_size = label.size * 0.6;
                text.sections[2].style.color = label.color.with_alpha(0.5);

                *transform =
                    Transform::from_translation(Vec3::new(label.offset.x, label.offset.y, label.z));
            }
        }
    }
}

/// there is no way to inherit position but not rotation from the parent entity transform yet
/// see: https://github.com/bevyengine/bevy/issues/1780
/// so labels will rotate with entities by default
fn fix_entity_label_rotations(
    mut q_text: Query<(&Parent, &mut Transform), With<EntityLabelChild>>,
    q_parents: Query<(&Transform, &EntityLabel), Without<EntityLabelChild>>,
) {
    for (parent, mut transform) in q_text.iter_mut() {
        if let Ok((parent_transform, fl)) = q_parents.get(parent.get()) {
            // global transform propagation system will make the rotation 0 now
            transform.rotation = parent_transform.rotation.inverse();
        }
    }
}
