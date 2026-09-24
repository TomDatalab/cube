//! Stable re-export of the pure-Rust model living in [`crate::test_fixtures`].
//!
//! The modules below started life as test fixtures for the planner. They are a
//! complete, JavaScript-free implementation of the `cube_bridge` traits over a
//! YAML data model, so they are promoted here (behind the default-on
//! `rust-model` feature) and consumed both by this crate's tests and by the
//! `cubeplanner` crate, which wraps them in a public planning API.

pub use crate::test_fixtures::cube_bridge::*;
