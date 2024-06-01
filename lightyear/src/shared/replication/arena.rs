use bevy::app::{App, Plugin};
use bevy::prelude::{ResMut, Resource};
use blink_alloc::{LocalBlinkAlloc, SyncBlinkAlloc};
use once_cell::sync::Lazy;
use parking_lot::{Mutex, RwLock};
use std::cell::{RefCell, UnsafeCell};
use std::sync::Arc;

pub static mut ARENA_ALLOCATOR: SyncBlinkAlloc = SyncBlinkAlloc::new();

// Marker resource to ensure that we don't mutable alias use the global allocator
#[derive(Resource, Default)]
pub struct ArenaManager;
