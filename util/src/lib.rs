//! # util
//!
//! Shared utilities for the odoru workspace.
//!
//! - [`frontmatter`] — parse YAML frontmatter from markdown files
//! - [`voice`] — load F5-TTS voice definitions from `voices/<name>/`
//! - [`cache`] — article cache in `~/.odoru/articles/`

pub mod frontmatter;
pub mod voice;
pub mod cache;
