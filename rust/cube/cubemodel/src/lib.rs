//! Cube data model loading, validation, view resolution and `/v1/meta`.
//!
//! This crate is the Rust replacement for the YAML half of
//! `@cubejs-backend/schema-compiler`. It loads a model directory, renders Jinja
//! templates, parses YAML, normalises keys the way `YamlCompiler` +
//! `camelizeCube` do, validates the result, resolves views into a flat member
//! list the way `CubeSymbols.prepareIncludes` does, and produces the `/v1/meta`
//! response body the API gateway returns.
//!
//! # Scope
//!
//! SQL evaluation and query planning are **out of scope**: every `sql` /
//! `sql_table` expression is kept verbatim as a [`String`] so a planner can
//! consume it later, but nothing in this crate parses or evaluates it.
//!
//! ## Behaviours intentionally not ported
//!
//! * **JavaScript / TypeScript (`.js`, `.ts`, …) data models.** This backend is
//!   pure Rust and embeds no scripting runtime, so `cube(...)` JS definitions
//!   cannot be evaluated. A model directory containing such a file is rejected
//!   with [`ModelError::UnsupportedModelFile`] naming the file, rather than
//!   silently skipping it.
//! * **Python (`cube.py`) template functions, filters and variables.** They
//!   need an embedded CPython interpreter, which this backend does not have.
//!   Jinja templating itself stays (minijinja is native Rust), but only the
//!   built-in `env_var` global and `COMPILE_CONTEXT` are in scope — a template
//!   that calls any other function fails with
//!   [`ModelError::Template`] instead of rendering as if it had worked.
//! * **`split: true` view includes.** Node synthesises extra split views; this
//!   crate skips those includes instead.
//! * **Joi's `Possible reasons (one of):` aggregation.** Validation reports one
//!   message per problem, each formatted like an individual Joi detail
//!   (`(<path> = <value>) <reason>`), rather than folding alternatives into a
//!   single multi-line message.
//! * **View groups.** `view_groups:` documents parse without error but are not
//!   modelled, so `rest_meta_response` never emits `viewGroups` at the top
//!   level of the response. Per-view `view_groups:` references are preserved on
//!   the view and surface as the `viewGroups` field of its meta config.
//! * **Access policies** are preserved as raw JSON; row-level filters are not
//!   evaluated here.
//!
//! # Example
//!
//! ```no_run
//! use cubemodel::{meta, ModelLoader};
//!
//! let model = ModelLoader::load_dir("model/cubes")?;
//! let config = meta::meta_config(&model);
//! let body = meta::rest_meta_response(&config, false);
//! # Ok::<(), cubemodel::ModelError>(())
//! ```

pub mod error;
pub mod formats;
pub mod jinja;
pub mod join_graph;
pub mod loader;
pub mod meta;
pub mod model;
pub mod naming;
pub mod validate;
pub mod views;
pub mod yaml;

pub use error::{ErrorReporter, ModelError, ModelErrorItem};
pub use jinja::TemplateContext;
pub use loader::ModelLoader;
pub use meta::{meta_config, rest_meta_response, CubeMetaConfig, MetaConfig};
pub use model::{
    CubeDef, DataModel, Dimension, EvaluatedFolder, EvaluatedHierarchy, Granularity, Hierarchy,
    IncludedMember, Join, Measure, PreAggregation, Segment,
};
