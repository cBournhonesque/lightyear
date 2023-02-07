use std::ops::{Deref, DerefMut};

/// Tracks which Properties have changed and need to be queued for syncing with
/// the Client
pub trait PropertyMutate: PropertyMutateClone + Send + Sync + 'static {
    /// Given the index of the Property whose value has changed, queue that
    /// Property for transmission to the Client
    fn mutate(&mut self, property_index: u8);
}

pub trait PropertyMutateClone {
    /// Clone ..
    fn clone_box(&self) -> Box<dyn PropertyMutate>;
}

impl<T: 'static + Clone + PropertyMutate> PropertyMutateClone for T {
    fn clone_box(&self) -> Box<dyn PropertyMutate> {
        Box::new(self.clone())
    }
}

impl Clone for Box<dyn PropertyMutate> {
    fn clone(&self) -> Box<dyn PropertyMutate> {
        PropertyMutateClone::clone_box(self.as_ref())
    }
}

#[derive(Clone)]
pub struct PropertyMutator {
    inner: Box<dyn PropertyMutate>,
}

impl PropertyMutator {
    pub fn new<M: PropertyMutate>(mutator: M) -> Self {
        let inner = Box::new(mutator);
        Self { inner }
    }

    pub fn clone_new(&self) -> Self {
        //let current_inner: &dyn PropertyMutateClone = self.inner.as_ref() as &dyn
        // PropertyMutateClone;
        let new_inner = self.inner.as_ref().clone_box();

        Self { inner: new_inner }
    }
}

impl Deref for PropertyMutator {
    type Target = dyn PropertyMutate;

    fn deref(&self) -> &Self::Target {
        self.inner.deref()
    }
}

impl DerefMut for PropertyMutator {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner.deref_mut()
    }
}
