use avian2d::prelude::{Position, Rotation};
use bevy::prelude::TransformSystem::TransformPropagate;
/// Utility plugin to display a text label next to an entity.
///
/// Label will track parent position, ignoring rotation.
use bevy::prelude::*;
use bevy::text::TextReader;
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
        app.add_systems(
            PostUpdate,
            (label_added, label_changed, fix_entity_label_rotations)
                .chain()
                .before(TransformPropagate),
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
    for (e, label) in q.iter() {
        commands
            .spawn((
                EntityLabelChild,
                TextLayout {
                    justify: JustifyText::Center,
                    linebreak: LineBreak::NoWrap,
                },
                Text2d(label.text.clone()),
                TextFont::from_font_size(label.size),
                TextColor(label.color),
                Transform::from_translation(Vec3::new(label.offset.x, label.offset.y, label.z)),
            ))
            .set_parent(e)
            .with_children(|builder| {
                builder.spawn((
                    TextSpan(label.sub_text.clone()),
                    TextFont::from_font_size(label.size * 0.85),
                    TextColor(label.color.with_alpha(0.6)),
                ));
            });
    }
}

/// modify text when EntityLabel changes
fn label_changed(
    q_parents: Query<(&EntityLabel, &Children), Changed<EntityLabel>>,
    mut text_writer: Text2dWriter,
    mut q_children: Query<&mut Transform, (With<EntityLabelChild>, Without<EntityLabel>)>,
) {
    for (label, children) in q_parents.iter() {
        for &child in children.iter() {
            if let Ok(mut transform) = q_children.get_mut(child) {
                if text_writer.text(child, 0).as_str() != label.text {
                    *text_writer.text(child, 0) = label.text.clone();
                }
                text_writer.font(child, 0).font_size = label.size;
                text_writer.color(child, 0).0 = label.color;

                if text_writer.text(child, 1).as_str() != label.sub_text {
                    *text_writer.text(child, 1) = label.sub_text.clone();
                }
                text_writer.font(child, 1).font_size = label.size * 0.85;
                text_writer.color(child, 1).0 = label.color.with_alpha(0.6);

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
    mut q_text: Query<(&ChildOf, &mut Transform), With<EntityLabelChild>>,
    q_parents: Query<(&Transform, &EntityLabel), Without<EntityLabelChild>>,
) {
    for (parent, mut transform) in q_text.iter_mut() {
        if let Ok((parent_transform, fl)) = q_parents.get(parent.get()) {
            // global transform propagation system will make the rotation 0 now
            transform.rotation = parent_transform.rotation.inverse();
        }
    }
}
