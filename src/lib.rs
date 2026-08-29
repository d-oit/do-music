//! do-music library: deterministic planning, serialization and assembly logic.
//!
//! The binary (`src/main.rs`) owns the CLI and orchestration; every piece of
//! logic that must be testable without a live API lives here: duration
//! parsing, track planning, assembly planning, API serialization, `.env`
//! handling, templates, video planning and FFmpeg arg building.

pub mod api;
pub mod assembly;
pub mod config;
pub mod duration;
pub mod plan;
pub mod providers;
pub mod render;
pub mod templates;
pub mod video;
pub mod visual;
