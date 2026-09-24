pub mod cube_bridge;
pub mod logical_plan;
pub mod physical_plan;
pub mod physical_plan_builder;
pub mod planner;
/// Pure-Rust YAML model and `cube_bridge` implementations.
///
/// Historically these were test-only fixtures; they are now the production
/// model used by the `cubeplanner` crate, re-exported under a stable name.
/// Enabled by the default `rust-model` feature.
#[cfg(any(test, feature = "rust-model"))]
pub mod rust_model;
#[cfg(any(test, feature = "rust-model"))]
pub mod test_fixtures;
#[cfg(test)]
mod tests;
pub(crate) mod utils;
