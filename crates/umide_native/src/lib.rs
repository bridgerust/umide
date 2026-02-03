//! Native GPU surface implementation for UMIDE
//! 
//! This crate provides cross-platform GPU texture sharing for embedding
//! emulator/simulator output directly into wgpu-rendered views.

pub mod wgpu_texture;
pub mod emulator;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "macos")]
pub mod simulator;

#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(target_os = "macos")]
pub use wgpu_texture::*;

#[cfg(target_os = "macos")]
pub use simulator::*;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum NativeSurfaceError {
    #[error("Failed to create native surface: {0}")]
    CreationFailed(String),
    
    #[error("Surface operation failed: {0}")]
    OperationFailed(String),
    
    #[error("Platform not supported")]
    PlatformNotSupported,
}

/// Pixel format for the surface
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceFormat {
    Rgba8 = 0,
    Bgra8 = 1,
}

/// Trait for native GPU surfaces that can be shared
pub trait GpuSurface: Send + Sync {
    /// Get the width of the surface
    fn width(&self) -> u32;
    
    /// Get the height of the surface
    fn height(&self) -> u32;
    
    /// Resize the surface
    fn resize(&mut self, width: u32, height: u32) -> Result<(), NativeSurfaceError>;
}
