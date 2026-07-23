//! Difference operations used by Lightyear's prediction correction.
//!
//! Network delta replication is handled separately by Replicon's
//! `Diffable` trait and `replicate_diff()` registration.

/// A value whose difference can be computed and applied.
///
/// `Delta` can be the value itself or a smaller type tailored to the changes
/// between two values.
pub trait Diffable<Delta = Self>: Clone {
    /// Returns the baseline value used when there is no previous value.
    fn base_value() -> Self;

    /// Computes the difference from `self` to `new` (`new - self`).
    fn diff(&self, new: &Self) -> Delta;

    /// Applies `delta` to `self` (`self + delta`).
    fn apply_diff(&mut self, delta: &Delta);
}
