//! Playback and platform adapter implementations.

pub mod playback;

pub use playback::{probe_container_duration_ms, FakePlaybackBackend, GStreamerBackend};
