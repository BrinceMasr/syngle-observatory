//! The Australian government data lookup behind `syngle-map gov ...` (Syngle-Ecosystem #628 to #630).
//!
//! The official extracts are read as streams into a snapshot (`snapshots/<data version>/`): sorted tables of fixed-width
//! entries and record files, read by position (no memory map: `unsafe` is forbidden here), so one call opens only what its
//! verb needs and costs a few small reads. The data version is a hash of the source files, every answer states it with each
//! source's as-of date, licence and attribution, and the same query on the same version gives the same bytes. See
//! `docs/gov.md`.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod abnlive;
mod abr;
mod abs;
mod absfetch;
mod address;
mod area;
mod asgs;
mod asic;
mod build;
mod ckan;
mod cli;
mod codec;
mod dates;
mod download;
mod envelope;
mod error;
mod geoms;
mod gnaf;
mod json;
mod names;
mod names_build;
mod net;
mod norm;
mod online;
mod query;
mod rd;
mod refresh;
mod snap;
mod sort;
mod sources;
mod stats;
mod store;
mod table;
mod tokens;
mod xml;

pub use cli::{Env, Online, Outcome, run, run_with};
pub use error::GovError;
pub use net::Net;
