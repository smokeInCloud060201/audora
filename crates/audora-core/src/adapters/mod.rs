//! Playback and platform adapter implementations.

pub mod playback;

pub use playback::{FakePlaybackBackend, GStreamerBackend};
