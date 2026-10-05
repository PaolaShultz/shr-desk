//! Optional winit/wgpu backend. Window creation occurs ONLY in explicit native mode.
use crate::{
    frontend::{Config, Event, Frontend},
    raster,
    render::{HEIGHT, Scene, WIDTH},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use winit::{
    application::ApplicationHandler,
    event::{ElementState, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{Key, NamedKey},
    window::{Window, WindowId},
};

/// Aspect-preserving viewport. Fractional downscale is necessary below full HD;
/// at larger sizes use an integer scale to preserve the bitmap cell rhythm.
pub fn viewport(width: u32, height: u32) -> (f32, f32, f32, f32) {
    if width == 0 || height == 0 {
        return (0.0, 0.0, 0.0, 0.0);
    }
    let fit = (width as f32 / WIDTH as f32).min(height as f32 / HEIGHT as f32);
    let scale = if fit >= 1.0 { fit.floor() } else { fit };
    let w = WIDTH as f32 * scale;
    let h = HEIGHT as f32 * scale;
    (
        ((width as f32 - w) / 2.0).floor(),
        ((height as f32 - h) / 2.0).floor(),
        w,
        h,
    )
}

const SHADER: &str = r#"
@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var<uniform> fit: vec4<f32>;
struct Vertex { @builtin(position) position: vec4<f32> }
@vertex fn vs(@builtin(vertex_index) i:u32)->Vertex {
    var xy=array<vec2<f32>,3>(vec2<f32>(-1.0,-1.0),vec2<f32>(3.0,-1.0),vec2<f32>(-1.0,3.0));
    var v:Vertex; v.position=vec4<f32>(xy[i],0.0,1.0); return v;
}
@fragment fn fs(v:Vertex)->@location(0) vec4<f32> {
    let size=textureDimensions(image);
    let source=(v.position.xy-fit.xy)*vec2<f32>(size)/fit.zw;
    let p=min(vec2<u32>(source),size-vec2<u32>(1)); return textureLoad(image,p,0);
}
"#;
struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    texture: wgpu::Texture,
    viewport_buffer: wgpu::Buffer,
    bind: wgpu::BindGroup,
    pipeline: wgpu::RenderPipeline,
    lost: Arc<AtomicBool>,
}
impl Gpu {
    async fn new(adapter: &wgpu::Adapter, format: wgpu::TextureFormat) -> Result<Self, String> {
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("Desk renderer"),
                    required_features: wgpu::Features::empty(),
                    // Keep conservative feature limits while supporting the adapter's
                    // actual presentation resolution, including enlarged viewports.
                    required_limits: wgpu::Limits::downlevel_defaults()
                        .using_resolution(adapter.limits()),
                },
                None,
            )
            .await
            .map_err(|e| e.to_string())?;
        let lost = Arc::new(AtomicBool::new(false));
        let flag = lost.clone();
        device.set_device_lost_callback(move |_, _| {
            flag.store(true, Ordering::Release);
        });
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Terminus CPU scene"),
            size: wgpu::Extent3d {
                width: WIDTH,
                height: HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let viewport_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Aspect-fit pixel mapping"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(
                        &texture.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: viewport_buffer.as_entire_binding(),
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Desk scene presentation"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs",
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs",
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
        });
        Ok(Self {
            device,
            queue,
            texture,
            viewport_buffer,
            bind,
            pipeline,
            lost,
        })
    }
    fn render(&self, scene: &Scene, target: &wgpu::TextureView, width: u32, height: u32) {
        let (x, y, w, h) = viewport(width, height);
        let mut fit = [0u8; 16];
        for (i, value) in [x, y, w, h].into_iter().enumerate() {
            fit[i * 4..(i + 1) * 4].copy_from_slice(&value.to_le_bytes());
        }
        self.queue.write_buffer(&self.viewport_buffer, 0, &fit);
        self.queue.write_texture(
            self.texture.as_image_copy(),
            &raster::rgba(scene),
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(WIDTH * 4),
                rows_per_image: Some(HEIGHT),
            },
            wgpu::Extent3d {
                width: WIDTH,
                height: HEIGHT,
                depth_or_array_layers: 1,
            },
        );
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
            });
            pass.set_viewport(x, y, w, h, 0.0, 1.0);
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([encoder.finish()]);
    }
}
struct WindowGpu {
    _instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    gpu: Gpu,
    config: wgpu::SurfaceConfiguration,
}
impl WindowGpu {
    async fn new(window: Arc<Window>) -> Result<Self, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN | wgpu::Backends::GL,
            ..Default::default()
        });
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| e.to_string())?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: Some(&surface),
                power_preference: wgpu::PowerPreference::LowPower,
                force_fallback_adapter: false,
            })
            .await
            .ok_or("no window adapter")?;
        let size = window.inner_size();
        let config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or("surface configuration unavailable")?;
        let gpu = Gpu::new(&adapter, config.format).await?;
        surface.configure(&gpu.device, &config);
        Ok(Self {
            _instance: instance,
            surface,
            gpu,
            config,
        })
    }
    fn resize(&mut self, w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        let max = self.gpu.device.limits().max_texture_dimension_2d;
        self.config.width = w.min(max);
        self.config.height = h.min(max);
        self.surface.configure(&self.gpu.device, &self.config);
    }
    fn present(&mut self, scene: &Scene) -> Result<(), wgpu::SurfaceError> {
        let frame = self.surface.get_current_texture()?;
        self.gpu.render(
            scene,
            &frame.texture.create_view(&Default::default()),
            self.config.width,
            self.config.height,
        );
        frame.present();
        Ok(())
    }
}
struct App {
    front: Frontend,
    window: Option<Arc<Window>>,
    gpu: Option<WindowGpu>,
    next: Instant,
    error: Option<String>,
    retry: Instant,
}
impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            match event_loop.create_window(
                Window::default_attributes()
                    .with_title("SHR Desk — real GP03 provider")
                    .with_inner_size(winit::dpi::PhysicalSize::new(WIDTH, HEIGHT)),
            ) {
                Ok(w) => self.window = Some(Arc::new(w)),
                Err(e) => {
                    self.error = Some(e.to_string());
                    event_loop.exit();
                    return;
                }
            }
        }
        if self.gpu.is_none() && Instant::now() >= self.retry {
            match pollster::block_on(WindowGpu::new(self.window.as_ref().unwrap().clone())) {
                Ok(g) => {
                    self.gpu = Some(g);
                    let _ = self.front.enqueue(Event::DeviceRestored);
                }
                Err(e) => {
                    self.retry = Instant::now() + Duration::from_secs(2);
                    self.front.message = format!("graphics unavailable: {e}; retry on resume");
                    let _ = self.front.enqueue(Event::DeviceLost);
                }
            }
        }
    }
    fn suspended(&mut self, _: &ActiveEventLoop) {
        let _ = self.front.enqueue(Event::DeviceLost);
        self.gpu = None;
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Focused(value) => {
                let _ = self.front.enqueue(Event::Focus(value));
            }
            WindowEvent::Resized(size) => {
                let _ = self.front.enqueue(Event::Resize(size.width, size.height));
                if let Some(g) = &mut self.gpu {
                    g.resize(size.width, size.height);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if !event.repeat
                    && let Some(key) = key_name(&event.logical_key)
                {
                    let _ = self.front.enqueue(Event::Key {
                        key,
                        pressed: event.state == ElementState::Pressed,
                    });
                }
            }
            WindowEvent::RedrawRequested => {
                self.front.pump();
                if self.front.width == 0 || self.front.height == 0 {
                    return;
                }
                if self
                    .gpu
                    .as_ref()
                    .is_some_and(|g| g.gpu.lost.load(Ordering::Acquire))
                {
                    let _ = self.front.enqueue(Event::DeviceLost);
                    self.gpu = None;
                    self.resumed(event_loop);
                }
                if let Some(g) = &mut self.gpu {
                    match g.present(&self.front.scene()) {
                        Ok(()) => {
                            self.front.mark_presented();
                        }
                        Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                            let _ = self.front.enqueue(Event::DeviceLost);
                            g.resize(self.front.width, self.front.height);
                            let _ = self.front.enqueue(Event::DeviceRestored);
                        }
                        Err(wgpu::SurfaceError::Timeout) => {
                            self.front.message = "graphics timeout; provider unaffected".into()
                        }
                        Err(wgpu::SurfaceError::OutOfMemory) => {
                            self.error = Some("graphics out of memory".into());
                            event_loop.exit();
                        }
                    }
                }
            }
            _ => {}
        }
    }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_none() && self.window.is_some() && Instant::now() >= self.retry {
            self.resumed(event_loop);
        }
        self.front.pump();
        if Instant::now() >= self.next {
            if let Some(w) = &self.window {
                w.request_redraw();
            }
            self.next = Instant::now() + Duration::from_millis(50);
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next));
    }
}
fn key_name(key: &Key) -> Option<String> {
    Some(match key {
        Key::Character(s) => s.to_string(),
        Key::Named(k) => match k {
            NamedKey::ArrowLeft => "Left",
            NamedKey::ArrowRight => "Right",
            NamedKey::ArrowUp => "Up",
            NamedKey::ArrowDown => "Down",
            NamedKey::Tab => "Tab",
            NamedKey::PageUp => "PageUp",
            NamedKey::PageDown => "PageDown",
            NamedKey::Enter => "Enter",
            NamedKey::Backspace => "Backspace",
            NamedKey::Escape => "Esc",
            NamedKey::F1 => "F1",
            NamedKey::F2 => "F2",
            NamedKey::F3 => "F3",
            NamedKey::F4 => "F4",
            NamedKey::F5 => "F5",
            NamedKey::F8 => "F8",
            NamedKey::F6 => "F6",
            NamedKey::F7 => "F7",
            NamedKey::F9 => "F9",
            NamedKey::F10 => "F10",
            _ => return None,
        }
        .into(),
        _ => return None,
    })
}
pub fn run(config: Config, role: Option<crate::roles::Config>) -> Result<(), String> {
    run_with_processing(config, role, false)
}
pub fn run_with_processing(
    config: Config,
    role: Option<crate::roles::Config>,
    processing: bool,
) -> Result<(), String> {
    run_with_capabilities(config, role, processing, false)
}
pub fn run_with_capabilities(
    config: Config,
    role: Option<crate::roles::Config>,
    processing: bool,
    brain_audio: bool,
) -> Result<(), String> {
    let event_loop = EventLoop::new().map_err(|e| e.to_string())?;
    let mut front = Frontend::new(config);
    if brain_audio {
        front.enable_brain_audio()?;
    }
    front.require_role();
    if let Some(role) = role {
        front.attach_role(role);
    }
    if processing {
        front.enable_processing()?;
    }
    let mut app = App {
        front,
        window: None,
        gpu: None,
        next: Instant::now(),
        error: None,
        retry: Instant::now(),
    };
    event_loop.run_app(&mut app).map_err(|e| e.to_string())?;
    if let Some(e) = app.error {
        Err(e)
    } else {
        Ok(())
    }
}
/// Explicit CPU Vulkan ICD only. Does not create an event loop, display or surface.
/// Caller sets VK_DRIVER_FILES to Mesa lavapipe's exact ICD path for this process.
pub fn offscreen(scene: &Scene) -> Result<String, String> {
    offscreen_at(scene, WIDTH, HEIGHT)
}
/// Exercise the same aspect-fit presentation at an explicit headless viewport.
pub fn offscreen_at(scene: &Scene, width: u32, height: u32) -> Result<String, String> {
    let driver = std::env::var("VK_DRIVER_FILES").map_err(
        |_| "offscreen requires explicit VK_DRIVER_FILES=/usr/share/vulkan/icd.d/lvp_icd.json",
    )?;
    if driver != "/usr/share/vulkan/icd.d/lvp_icd.json" {
        return Err("offscreen only permits the explicit lavapipe CPU ICD".into());
    }
    if std::env::var_os("VK_ADD_DRIVER_FILES").is_some()
        || std::env::var("VK_ICD_FILENAMES")
            .is_ok_and(|v| v != "/usr/share/vulkan/icd.d/lvp_icd.json")
    {
        return Err("offscreen refuses additional/conflicting Vulkan ICDs".into());
    }
    if width == 0 || height == 0 {
        return Ok("CPU offscreen suspended: zero-size viewport".into());
    }
    pollster::block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                force_fallback_adapter: true,
                compatible_surface: None,
                power_preference: wgpu::PowerPreference::LowPower,
            })
            .await
            .ok_or("CPU adapter unavailable")?;
        let info = adapter.get_info();
        if info.device_type != wgpu::DeviceType::Cpu {
            return Err("non-CPU adapter refused".into());
        }
        let gpu = Gpu::new(&adapter, wgpu::TextureFormat::Rgba8Unorm).await?;
        gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        gpu.render(
            scene,
            &target.create_view(&Default::default()),
            width,
            height,
        );
        let row_bytes = (width * 4).div_ceil(256) * 256;
        let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(row_bytes) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::ImageCopyBuffer {
                buffer: &buffer,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(row_bytes),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        gpu.queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        gpu.device.poll(wgpu::Maintain::Wait);
        rx.recv_timeout(Duration::from_secs(10))
            .map_err(|_| "GPU readback deadline")?
            .map_err(|e| e.to_string())?;
        let mapped = buffer.slice(..).get_mapped_range();
        let expected = raster::rgba(scene);
        let (vx, vy, vw, vh) = viewport(width, height);
        for y in 0..height as usize {
            for x in 0..width as usize {
                let px = x as f32 + 0.5;
                let py = y as f32 + 0.5;
                let wanted = if px >= vx && px < vx + vw && py >= vy && py < vy + vh {
                    let sx = (((px - vx) * WIDTH as f32 / vw) as usize).min(WIDTH as usize - 1);
                    let sy = (((py - vy) * HEIGHT as f32 / vh) as usize).min(HEIGHT as usize - 1);
                    &expected[(sy * WIDTH as usize + sx) * 4..(sy * WIDTH as usize + sx + 1) * 4]
                } else {
                    &[0, 0, 0, 255]
                };
                let offset = y * row_bytes as usize + x * 4;
                if &mapped[offset..offset + 4] != wanted {
                    return Err(format!(
                        "GPU/CPU aspect-fit raster mismatch at {x},{y} in {width}x{height}"
                    ));
                }
            }
        }
        drop(mapped);
        buffer.unmap();
        if let Some(e) = gpu.device.pop_error_scope().await {
            return Err(e.to_string());
        }
        Ok(format!(
            "CPU offscreen exact RGBA match: {} ({:?}), {}x{}",
            info.name, info.backend, width, height
        ))
    })
}
