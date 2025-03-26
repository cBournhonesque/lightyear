
pub trait LeafwingUserAction:
    UserAction + Copy + Actionlike + bevy::reflect::GetTypeRegistration
{
}

impl<A: UserAction + Copy + Actionlike + bevy::reflect::GetTypeRegistration> LeafwingUserAction
    for A
{
}

impl<A: LeafwingUserAction> UserActionState for ActionState<A> {
    type UserAction = A;
}
