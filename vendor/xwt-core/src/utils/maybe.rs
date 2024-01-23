#[cfg(not(target_family = "wasm"))]
pub trait Send: std::marker::Send {}

#[cfg(not(target_family = "wasm"))]
impl<T> self::Send for T where T: std::marker::Send {}

#[cfg(target_family = "wasm")]
pub trait Send {}

#[cfg(target_family = "wasm")]
impl<T> self::Send for T {}

#[cfg(not(target_family = "wasm"))]
pub trait Sync: std::marker::Sync {}

#[cfg(not(target_family = "wasm"))]
impl<T> self::Sync for T where T: std::marker::Sync {}

#[cfg(target_family = "wasm")]
pub trait Sync {}

#[cfg(target_family = "wasm")]
impl<T> self::Sync for T {}
