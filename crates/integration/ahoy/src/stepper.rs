use avian3d::prelude::Position;
use bevy_ahoy::{
    CharacterControllerStepError, CharacterControllerStepper, CharacterLook,
    input::AccumulatedInput,
};
use bevy_ecs::{
    prelude::*,
    query::{QueryData, QueryEntityError},
    system::{ParamSet, SystemParam},
};
use bevy_time::{Fixed, Time};
use bevy_transform::components::Transform;
use core::{fmt, time::Duration};

/// Components touched by a conservative Ahoy KCC step.
#[derive(QueryData)]
#[query_data(mutable)]
pub struct AhoyStepParts {
    pub input: &'static mut AccumulatedInput,
    pub look: &'static mut CharacterLook,
    pub transform: &'static mut Transform,
    pub position: &'static mut Position,
}

/// Error returned when the integration cannot step an Ahoy controller.
#[derive(Debug)]
pub enum LightyearAhoyStepError {
    Query(QueryEntityError),
    Step(CharacterControllerStepError),
}

impl fmt::Display for LightyearAhoyStepError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Query(err) => write!(f, "failed to query Ahoy step components: {err}"),
            Self::Step(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for LightyearAhoyStepError {}

impl From<QueryEntityError> for LightyearAhoyStepError {
    fn from(value: QueryEntityError) -> Self {
        Self::Query(value)
    }
}

impl From<CharacterControllerStepError> for LightyearAhoyStepError {
    fn from(value: CharacterControllerStepError) -> Self {
        Self::Step(value)
    }
}

/// Manual Ahoy KCC stepping context.
///
/// Use this when prediction/rollback needs exact control over which input tick
/// is consumed before the KCC step. The stepper mirrors Ahoy's `Transform`
/// output into Avian `Position` immediately after every successful step.
#[derive(SystemParam)]
pub struct LightyearAhoyStepper<'w, 's> {
    set: ParamSet<
        'w,
        's,
        (
            CharacterControllerStepper<'w, 's>,
            Query<'w, 's, AhoyStepParts>,
        ),
    >,
    fixed_time: Res<'w, Time<Fixed>>,
}

impl LightyearAhoyStepper<'_, '_> {
    /// Duration of one fixed simulation tick.
    pub fn fixed_delta(&self) -> Duration {
        self.fixed_time.timestep()
    }

    /// Step an entity using the current `AccumulatedInput` and `CharacterLook`.
    pub fn step_entity(&mut self, entity: Entity) -> Result<(), LightyearAhoyStepError> {
        let fixed_delta = self.fixed_delta();
        self.set.p0().step_entity(entity, fixed_delta)?;
        self.mirror_transform_to_position(entity)?;
        Ok(())
    }

    /// Mutate Ahoy input/look state, then step the entity once.
    pub fn step_entity_with_input(
        &mut self,
        entity: Entity,
        mut apply_input: impl FnMut(Duration, &mut AccumulatedInput, &mut CharacterLook),
    ) -> Result<(), LightyearAhoyStepError> {
        let fixed_delta = self.fixed_delta();
        {
            let mut query = self.set.p1();
            let mut parts = query.get_mut(entity)?;
            apply_input(fixed_delta, &mut parts.input, &mut parts.look);
        }
        self.set.p0().step_entity(entity, fixed_delta)?;
        self.mirror_transform_to_position(entity)?;
        Ok(())
    }

    /// Mirror the current `Transform.translation` into Avian `Position`.
    pub fn mirror_transform_to_position(
        &mut self,
        entity: Entity,
    ) -> Result<(), LightyearAhoyStepError> {
        let mut query = self.set.p1();
        let mut parts = query.get_mut(entity)?;
        parts.position.0 = parts.transform.translation;
        Ok(())
    }
}
