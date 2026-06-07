//! # util
//!
//! Shared utilities for the odoru workspace.
//!
//! - [`frontmatter`] — parse YAML frontmatter from markdown files
//! - [`voice`] — load F5-TTS voice definitions from `voices/<name>/`
//! - [`documents`] — UUID-keyed document store in `~/.odoru/documents/<uuid>/`
//! - [`index`] — in-memory source_url + content_hash indexes
//! - [`slug`] — title-to-slug conversion and export directory name helpers

pub mod frontmatter;
pub mod voice;
pub mod documents;
pub mod index;
pub mod slug;
