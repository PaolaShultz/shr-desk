//! Surface-only offline foundation. No device, network, audio or DSP ownership.
pub mod actions;
pub mod console;
pub mod midi;
pub mod model;
pub mod provider;
pub mod render;

pub mod audio;

pub mod frontend;
pub mod local_audio;
#[cfg(feature = "native")]
pub mod native;
pub mod raster;

pub mod roles;

pub mod modules;

pub mod processing;

pub mod scopes;

pub mod topology;

pub mod structure;

pub mod pages;

pub mod remote;
