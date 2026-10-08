//! Standalone verification harness; the integration crate owns its manifest.
#![deny(clippy::unwrap_used)]
#[path="../../../libcram-core/src/entropy/mod.rs"]
pub mod entropy;
#[path="../../../libcram-core/src/telemetry/mod.rs"]
pub mod telemetry;
#[cfg(test)]
#[path="../../src/wire.rs"]
mod wire;
