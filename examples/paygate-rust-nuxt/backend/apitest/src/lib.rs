//! The cucumber-rs harness for `spec/bdd/api/*.feature`. See `tests/api.rs`
//! for the entry point; this crate holds the [`World`], the per-scenario
//! [`stack`], the step implementations, and the registry-parity checks.

pub mod registry;
pub mod stack;
pub mod steps;
pub mod world;

pub use world::World;
