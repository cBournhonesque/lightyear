pub trait Named {
    /// Gets the String representation of the Type of the Component, used for debugging
    fn name(&self) -> String;
}
