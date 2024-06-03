//! Captures trick to allow for better impl Iterator + 'a
//! See https://rust-lang.github.io/rfcs/3498-lifetime-capture-rules-2024.html
//! and https://www.youtube.com/watch?v=CWiz_RtA1Hw

pub(crate) trait Captures<U> {}
impl<T: ?Sized, U> Captures<U> for T {}
