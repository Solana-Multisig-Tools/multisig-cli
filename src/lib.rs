#![deny(clippy::unwrap_used, clippy::expect_used)]

pub mod application;
pub mod cli;
pub mod domain;
pub mod error;
pub mod infra;
pub mod output;
pub mod sanitize;
#[cfg(feature = "tui")]
pub mod tui;

#[cfg(feature = "instruction-builder")]
pub mod instruction_builder;
