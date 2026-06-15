use bevy_ecs::query::QueryData;
use core::fmt::Debug;
use leafwing_input_manager::Actionlike;
use leafwing_input_manager::action_state::ActionState;
use lightyear_inputs::input_message::ActionStateQueryData;
use serde::Serialize;
use serde::de::DeserializeOwned;

pub trait LeafwingUserAction:
    Serialize
    + DeserializeOwned
    + Clone
    + PartialEq
    + Send
    + Sync
    + Debug
    + 'static
    + Copy
    + Actionlike
    + bevy_reflect::GetTypeRegistration
{
}

impl<
    A: Serialize
        + DeserializeOwned
        + Clone
        + PartialEq
        + Send
        + Sync
        + Debug
        + 'static
        + Copy
        + Actionlike
        + bevy_reflect::GetTypeRegistration,
> LeafwingUserAction for A
{
}

#[derive(QueryData, Debug)]
#[query_data(mutable)]
/// To bypass the orphan rule, we wrap the ActionState from leafwing_input_manager
pub struct ActionStateWrapper<A: LeafwingUserAction> {
    pub(crate) inner: &'static mut ActionState<A>,
}

impl<A: LeafwingUserAction> ActionStateQueryData for ActionStateWrapper<A> {
    type Mut = Self;
    type MutItemInner<'w> = &'w mut ActionState<A>;
    type Main = ActionState<A>;
    type Bundle = ActionState<A>;

    fn as_read_only<'a, 'w: 'a, 's>(
        state: &'a <Self::Mut as QueryData>::Item<'w, 's>,
    ) -> <<Self::Mut as QueryData>::ReadOnly as QueryData>::Item<'a, 's> {
        ActionStateWrapperReadOnlyItem {
            inner: &state.inner,
        }
    }

    fn into_inner<'w, 's>(
        mut_item: <Self::Mut as QueryData>::Item<'w, 's>,
    ) -> Self::MutItemInner<'w> {
        mut_item.inner.into_inner()
    }

    fn as_mut(bundle: &mut Self::Bundle) -> Self::MutItemInner<'_> {
        bundle
    }

    fn base_value() -> Self::Bundle {
        ActionState::<A>::default()
    }
}
