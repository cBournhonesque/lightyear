use core::fmt::Debug;
use bevy_ecs::query::QueryData;
use leafwing_input_manager::Actionlike;
use serde::Serialize;
use serde::de::DeserializeOwned;
use lightyear_inputs::input_message::ActionStateQueryData;
use leafwing_input_manager::action_state::ActionState;

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

#[derive(QueryData)]
#[query_data(mutable)]
/// To bypass the orphan rule, we wrap the ActionState from leafwing_input_manager
pub struct ActionStateWrapper<A: LeafwingUserAction>{
    pub(crate) inner: &'static mut ActionState<A>
}

impl<A: LeafwingUserAction> ActionStateQueryData for ActionStateWrapper<A> {
    type Mut = Self;
    type MutItemInner<'w> = &'w mut ActionState<A>;
    type Main = ActionState<A>;
    type Bundle = ActionState<A>;

    fn as_read_only<'w, 'a: 'w>(state: &'a ActionStateWrapperItem<'w, A>) -> ActionStateWrapperReadOnlyItem<'w, A> {
        ActionStateWrapperReadOnlyItem {
            inner: &state.inner,
        }
    }

    fn into_inner<'w>(mut_item: <Self::Mut as QueryData>::Item<'w>) -> Self::MutItemInner<'w> {
        mut_item.inner.into_inner()
    }

    fn as_mut<'w>(bundle: &'w mut Self::Bundle) -> Self::MutItemInner<'w> {
        bundle
    }

    fn base_value() -> Self::Bundle {
        ActionState::<A>::default()
    }
}