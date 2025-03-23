//! Contains a set of shared types

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod tick;


pub mod network;
pub mod time;
mod prediction;
mod history_buffer;