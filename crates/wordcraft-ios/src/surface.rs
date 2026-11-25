//! Metal surface management and GPU rendering for iPadOS.

use std::ffi::c_void;

#[cfg(any(target_os = "ios", target_os = "macos"))]
use egui_wgpu::{Renderer, RendererOptions, ScreenDescriptor, wgpu};

#[cfg(any(target_os = "ios", target_os = "macos"))]
struct GpuState {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    needs_reconfigure: bool,
}

/// Holds the native `CAMetalLayer` reference, viewport bounds, and active `wgpu` Metal pipeline.
pub struct MetalSurface {
    layer_ptr: *mut c_void,
    width: f32,
    height: f32,
    scale: f32,
    #[cfg(any(target_os = "ios", target_os = "macos"))]
    gpu: Option<GpuState>,
}

// SAFETY: The Metal layer pointer is provided by UIKit/CAMetalLayer from the main thread
// and accessed during render passes synchronized by the iOS display link.
unsafe impl Send for MetalSurface {}
unsafe impl Sync for MetalSurface {}

impl MetalSurface {
    /// Creates a new surface holder with dimensions.
    pub fn new(scale: f32, width: f32, height: f32) -> Self {
        Self {
            layer_ptr: std::ptr::null_mut(),
            width,
            height,
            scale,
            #[cfg(any(target_os = "ios", target_os = "macos"))]
            gpu: None,
        }
    }

    /// Attaches the native `CAMetalLayer` pointer.
    pub fn set_layer(&mut self, layer: *mut c_void) {
        self.layer_ptr = layer;
        #[cfg(any(target_os = "ios", target_os = "macos"))]
        {
            self.gpu = None;
        }
    }

    /// Returns the raw Metal layer pointer.
    pub fn layer(&self) -> *mut c_void {
        self.layer_ptr
    }

    /// Updates surface dimensions and scale factor.
    pub fn resize(&mut self, width: f32, height: f32, scale: f32) {
        self.width = width;
        self.height = height;
        self.scale = scale;
        #[cfg(any(target_os = "ios", target_os = "macos"))]
        {
            let (phys_w, phys_h) = self.physical_size();
            if let Some(gpu) = &mut self.gpu
                && (phys_w != gpu.config.width || phys_h != gpu.config.height)
            {
                gpu.config.width = phys_w;
                gpu.config.height = phys_h;
                gpu.needs_reconfigure = true;
            }
        }
    }

    /// Surface width in points.
    pub fn width(&self) -> f32 {
        self.width
    }

    /// Surface height in points.
    pub fn height(&self) -> f32 {
        self.height
    }

    /// Display scale factor (points to native pixels, e.g., 2.0 on Retina iPads).
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Whether a valid `CAMetalLayer` pointer and non-zero dimensions are available.
    pub fn is_ready(&self) -> bool {
        !self.layer_ptr.is_null() && self.width > 0.0 && self.height > 0.0
    }

    #[cfg(any(target_os = "ios", target_os = "macos"))]
    fn physical_size(&self) -> (u32, u32) {
        let s = if self.scale > 0.0 { self.scale } else { 2.0 };
        let w = ((self.width * s).round() as u32).max(1);
        let h = ((self.height * s).round() as u32).max(1);
        (w, h)
    }

    #[cfg(any(target_os = "ios", target_os = "macos"))]
    fn ensure_gpu(&mut self) -> Option<&mut GpuState> {
        if !self.is_ready() {
            return None;
        }
        if self.gpu.is_none() {
            let (phys_w, phys_h) = self.physical_size();
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::METAL,
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });

            // SAFETY: `self.layer_ptr` is a live `CAMetalLayer` owned by `MetalView`,
            // verified non-null by `is_ready()`, and outlives `MetalSurface`.
            let surface = match unsafe { instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(self.layer_ptr)) } {
                Ok(s) => s,
                Err(e) => {
                    log::error!("wordcraft-ios: failed to create Metal wgpu surface: {e}");
                    return None;
                }
            };

            let adapter = match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                ..Default::default()
            })) {
                Ok(a) => a,
                Err(e) => {
                    log::error!("wordcraft-ios: failed to find Metal GPU adapter: {e}");
                    return None;
                }
            };

            let (device, queue) = match pollster::block_on(
                adapter.request_device(&wgpu::DeviceDescriptor { label: Some("wordcraft_ios_device"), ..Default::default() }),
            ) {
                Ok(dq) => dq,
                Err(e) => {
                    log::error!("wordcraft-ios: failed to create Metal GPU device: {e}");
                    return None;
                }
            };

            let caps = surface.get_capabilities(&adapter);
            let format = caps
                .formats
                .iter()
                .copied()
                .find(|f| matches!(f, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm))
                .or_else(|| caps.formats.first().copied())
                .unwrap_or(wgpu::TextureFormat::Bgra8Unorm);

            let mut config = surface.get_default_config(&adapter, phys_w, phys_h).unwrap_or(wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format,
                color_space: wgpu::SurfaceColorSpace::Auto,
                width: phys_w,
                height: phys_h,
                present_mode: wgpu::PresentMode::AutoVsync,
                desired_maximum_frame_latency: 2,
                alpha_mode: wgpu::CompositeAlphaMode::Auto,
                view_formats: vec![],
            });
            config.format = format;
            config.width = phys_w;
            config.height = phys_h;
            surface.configure(&device, &config);

            let renderer = Renderer::new(&device, format, RendererOptions::default());
            self.gpu = Some(GpuState { device, queue, surface, config, renderer, needs_reconfigure: false });
        }
        self.gpu.as_mut()
    }

    /// Uploads egui textures/buffers and presents a rendered Metal frame to `CAMetalLayer`.
    #[cfg(any(target_os = "ios", target_os = "macos"))]
    pub fn render(&mut self, pixels_per_point: f32, textures_delta: &egui::TexturesDelta, clipped_primitives: &[egui::ClippedPrimitive]) {
        let Some(gpu) = self.ensure_gpu() else {
            return;
        };

        if gpu.needs_reconfigure {
            gpu.surface.configure(&gpu.device, &gpu.config);
            gpu.needs_reconfigure = false;
        }

        for (id, image_deltas) in &textures_delta.set {
            for image_delta in image_deltas {
                gpu.renderer.update_texture(&gpu.device, &gpu.queue, *id, image_delta);
            }
        }

        let screen_descriptor = ScreenDescriptor {
            size_in_pixels: [gpu.config.width, gpu.config.height],
            pixels_per_point: if pixels_per_point > 0.0 { pixels_per_point } else { 1.0 },
        };

        let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("wordcraft_egui_encoder") });
        let user_cmd_bufs = gpu.renderer.update_buffers(&gpu.device, &gpu.queue, &mut encoder, clipped_primitives, &screen_descriptor);

        let frame = match gpu.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => frame,
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                gpu.needs_reconfigure = true;
                frame
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                gpu.surface.configure(&gpu.device, &gpu.config);
                for id in &textures_delta.free {
                    gpu.renderer.free_texture(id);
                }
                return;
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded | wgpu::CurrentSurfaceTexture::Validation => {
                for id in &textures_delta.free {
                    gpu.renderer.free_texture(id);
                }
                return;
            }
        };

        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        {
            let render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("wordcraft_egui_render_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.95, g: 0.96, b: 0.97, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            gpu.renderer.render(&mut render_pass.forget_lifetime(), clipped_primitives, &screen_descriptor);
        }

        let encoded = encoder.finish();
        gpu.queue.submit(user_cmd_bufs.into_iter().chain(std::iter::once(encoded)));
        for id in &textures_delta.free {
            gpu.renderer.free_texture(id);
        }
        gpu.queue.present(frame);
    }

    /// No-op render stub when running host unit tests on non-Apple platforms.
    #[cfg(not(any(target_os = "ios", target_os = "macos")))]
    pub fn render(&mut self, _pixels_per_point: f32, _textures_delta: &egui::TexturesDelta, _clipped_primitives: &[egui::ClippedPrimitive]) {}
}
