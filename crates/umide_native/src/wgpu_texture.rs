//! wgpu texture integration for importing native surfaces
//!
//! This module provides utilities for importing IOSurface-backed Metal textures
//! into wgpu's rendering pipeline for zero-copy emulator display.

use wgpu::{Device, Texture, TextureDescriptor, TextureFormat, TextureUsages};

#[cfg(target_os = "macos")]
use crate::{GpuSurface, MacOSSurface};

/// Error type for texture import operations
#[derive(Debug, thiserror::Error)]
pub enum TextureImportError {
    #[error("Failed to import texture: {0}")]
    ImportFailed(String),

    #[error("Backend not supported for texture import")]
    UnsupportedBackend,

    #[error("Invalid surface state")]
    InvalidSurface,
}

/// A wrapper around a wgpu texture that was imported from a native surface
///
/// This allows zero-copy rendering of emulator/simulator output.
pub struct ImportedTexture {
    /// The wgpu texture handle
    pub texture: Texture,
    /// Width of the texture
    pub width: u32,
    /// Height of the texture
    pub height: u32,
}

impl ImportedTexture {
    /// Create a texture view for rendering
    pub fn create_view(&self) -> wgpu::TextureView {
        self.texture
            .create_view(&wgpu::TextureViewDescriptor::default())
    }
}

/// Import a MacOSSurface into wgpu as a texture
///
/// # Safety
///
/// This function requires that:
/// - The wgpu device was created with the Metal backend
/// - The surface's Metal texture is valid and not currently being written to
///
/// # Current Implementation
///
/// Due to wgpu not yet exposing stable APIs for importing external Metal textures,
/// this currently creates a new wgpu texture and provides a method to copy data
/// from the IOSurface. True zero-copy import will require wgpu HAL access.
#[cfg(target_os = "macos")]
pub fn import_surface_as_texture(
    device: &Device,
    surface: &MacOSSurface,
) -> Result<ImportedTexture, TextureImportError> {
    let width = surface.width();
    let height = surface.height();

    // Create a wgpu texture that we can copy IOSurface data into
    // In the future, this should use wgpu HAL to directly import the Metal texture
    let texture = device.create_texture(&TextureDescriptor {
        label: Some("emulator_surface"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: TextureFormat::Bgra8Unorm,
        usage: TextureUsages::TEXTURE_BINDING
            | TextureUsages::COPY_DST
            | TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });

    Ok(ImportedTexture {
        texture,
        width,
        height,
    })
}

/// Copy pixel data from an IOSurface to a wgpu texture
///
/// This is a temporary solution until true zero-copy import is available.
#[cfg(target_os = "macos")]
pub fn copy_surface_to_texture(
    queue: &wgpu::Queue,
    surface: &MacOSSurface,
    imported: &ImportedTexture,
) -> Result<(), TextureImportError> {
    // Lock the surface to get CPU access
    let lock = surface
        .lock()
        .map_err(|e| TextureImportError::ImportFailed(e.to_string()))?;

    let buffer_ptr = lock.buffer_ptr();
    if buffer_ptr.is_null() {
        return Err(TextureImportError::InvalidSurface);
    }

    let stride = surface.stride();
    let height = surface.height();
    let bytes_per_row = stride;
    let data_size = (bytes_per_row * height) as usize;

    // Safety: We have locked the surface, so the buffer is valid
    let data = unsafe { std::slice::from_raw_parts(buffer_ptr, data_size) };

    // Write to the wgpu texture
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &imported.texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(bytes_per_row),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width: imported.width,
            height: imported.height,
            depth_or_array_layers: 1,
        },
    );

    Ok(())
}

/// Helper to create a bind group for rendering the imported texture
pub fn create_texture_bind_group(
    device: &Device,
    layout: &wgpu::BindGroupLayout,
    imported: &ImportedTexture,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    let view = imported.create_view();

    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("emulator_texture_bind_group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

#[cfg(not(target_os = "macos"))]
pub fn import_surface_as_texture(
    _device: &Device,
    _width: u32,
    _height: u32,
) -> Result<ImportedTexture, TextureImportError> {
    Err(TextureImportError::UnsupportedBackend)
}
