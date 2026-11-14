//! The Australian government data lookup behind `syngle-map gov ...` (Syngle-Ecosystem #628 to #630).
//!
//! The official extracts are read as streams into a snapshot (`snapshots/<data version>/`): sorted tables of fixed-width
//! entries and record files, read by position (no memory map: `unsafe` is forbidden here), so one call opens only what its
//! verb needs and costs a few small reads. The data version is a hash of the source files, every answer states it with each
//! source's as-of date, licence and attribution, and the same query on the same version gives the same bytes. See
//! `docs/gov.md`.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod abr;
mod asic;
mod build;
mod cli;
mod codec;
mod envelope;
mod error;
mod json;
mod names;
mod names_build;
mod norm;
mod query;
mod rd;
mod refresh;
mod snap;
mod sort;
mod sources;
mod store;
mod table;
mod xml;

pub use cli::{Env, Outcome, run};
pub use error::GovError;
