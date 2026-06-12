//! The OpenCreate engine: Vulkan rendering via `ash` (ARCHITECTURE.md §4).
//!
//! Vulkan 1.2 baseline, constrained to what MoltenVK supports so macOS works
//! through the Vulkan-on-Metal translation layer. The renderer never sees game
//! logic; it consumes meshes and transforms.

mod bloom;
mod chunk_renderer;
mod clouds;
mod context;
mod depth;
mod entity;
mod exposure;
mod far_renderer;
mod hdr;
mod mesh;
mod font;
mod outline;
mod shadow;
mod sky_pass;
mod swapchain;
mod texture;
mod ui;

use anyhow::{Context as _, Result};
use ash::vk;
use glam::{DVec3, Mat4, Vec4};
use gpu_allocator::vulkan::{Allocator, AllocatorCreateDesc};
use oc_core::{BlockPos, SectionPos};
use raw_window_handle::{RawDisplayHandle, RawWindowHandle};

use bloom::BloomPass;
use chunk_renderer::ChunkRenderer;
use clouds::CloudLayer;
use context::VulkanContext;
use depth::DepthBuffer;
use entity::EntityRenderer;
use exposure::ExposurePass;
use far_renderer::FarRenderer;
use hdr::{HdrTarget, TonemapPass};
use outline::OutlineRenderer;
use shadow::ShadowPass;
use sky_pass::SkyPass;
use swapchain::Swapchain;
use ui::UiRenderer;

pub use far_renderer::{FarTile, FarVertex};
pub use mesh::{ChunkMesh, SectionMeshes, mesh_section};
pub use texture::block_swatch;
pub use entity::EntityDraw;
pub use ui::{UiQuad, UiText};

/// Number of frames the CPU may record ahead of the GPU.
const FRAMES_IN_FLIGHT: usize = 2;

/// Per-frame camera state, camera-relative (§3): `view_proj` contains no
/// translation; world-space translation happens in f64 against `position`.
pub struct FrameCamera {
    pub view_proj: Mat4,
    pub position: DVec3,
    /// Block to draw the targeting outline around, if any.
    pub highlight: Option<BlockPos>,
    /// xyz: direction toward the sun (normalized); w: ambient light level.
    pub sun: Vec4,
    /// Sky clear color for this frame (day/night cycle).
    pub sky_color: [f32; 4],
    /// Debug HUD text; empty hides the overlay.
    pub hud: String,
    /// Glyph scale for the HUD overlay (the client's effective UI scale).
    pub hud_scale: f32,
    /// Seconds since the client started (wave animation phase).
    pub time: f32,
    /// Overhead sky color (the dome blends horizon -> zenith); w: star
    /// visibility 0..1.
    pub sky_zenith: [f32; 4],
    /// xyz: unscaled sun direction; w: daylight 0..1 (sky dome).
    pub sky_sun: [f32; 4],
    /// rgb: horizon color opposite the sun (dusk darkens there first);
    /// w: moon phase 0..1.
    pub sky_away: [f32; 4],
    /// Celestial rotation angle, radians (stars turn with the day).
    pub sky_angle: f32,
    /// Where distance fog saturates, in blocks (~the render distance).
    pub fog_distance: f32,
    /// Draw the cloud layer this frame (graphics setting).
    pub clouds: bool,
    /// Sun shadows enabled (settings toggle).
    pub shadows: bool,
    /// Water reflects the scene (SSR; settings toggle).
    pub water_reflections: bool,
    /// Draw the coarse far-terrain ring beyond the loaded chunks.
    pub far_terrain: bool,
    /// Cloud slab color (rgb) + opacity (a) for the moment of day.
    pub cloud_color: [f32; 4],
    /// Solid UI rectangles (hotbar etc.), drawn under the text.
    pub ui_quads: Vec<UiQuad>,
    /// Positioned text runs (slot counts etc.).
    pub ui_texts: Vec<UiText>,
    /// Entities to draw this frame (placeholder cuboids).
    pub entities: Vec<EntityDraw>,
}

/// Renderer counters for the perf log (§11).
#[derive(Debug, Clone, Copy)]
pub struct RenderStats {
    /// Chunk meshes resident on the GPU.
    pub chunks_resident: usize,
    /// Chunks drawn last frame after frustum culling.
    pub chunks_drawn: u32,
}

/// Owns the Vulkan device, swapchain and per-frame state, and draws frames.
pub struct Renderer {
    ctx: VulkanContext,
    allocator: Option<Allocator>,
    swapchain: Swapchain,
    render_pass: vk::RenderPass,
    depth: DepthBuffer,
    framebuffers: Vec<vk::Framebuffer>,
    /// Offscreen HDR world target (stage A2); the world pass renders here.
    hdr: HdrTarget,
    tonemap: TonemapPass,
    /// World render resolution as a fraction of the window.
    resolution_scale: f32,
    scale_dirty: bool,
    chunks: ChunkRenderer,
    entity: EntityRenderer,
    outline: OutlineRenderer,
    sky: SkyPass,
    clouds_layer: CloudLayer,
    bloom: BloomPass,
    exposure: ExposurePass,
    shadow: ShadowPass,
    far: FarRenderer,
    ui: UiRenderer,
    command_pool: vk::CommandPool,
    command_buffers: Vec<vk::CommandBuffer>,
    /// Signalled when the acquired image is ready to be rendered to. Per frame in flight.
    image_available: Vec<vk::Semaphore>,
    /// Signalled when rendering to an image is done. Per swapchain image,
    /// because presentation waits on it after the frame's fence is reused.
    render_finished: Vec<vk::Semaphore>,
    in_flight: Vec<vk::Fence>,
    /// Monotonic frame counter; `frame % FRAMES_IN_FLIGHT` is the slot index,
    /// and the counter timestamps retired GPU buffers for safe destruction.
    frame: u64,
    /// Pending window size to recreate the swapchain at, set on resize.
    pending_extent: Option<vk::Extent2D>,
    chunks_drawn: u32,
}

impl Renderer {
    /// # Safety
    /// The window behind the raw handles must outlive the renderer.
    pub unsafe fn new(
        display: RawDisplayHandle,
        window: RawWindowHandle,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        unsafe {
            let ctx = VulkanContext::new(display, window)?;
            let mut allocator = Allocator::new(&AllocatorCreateDesc {
                instance: ctx.instance.clone(),
                device: ctx.device.clone(),
                physical_device: ctx.physical_device,
                debug_settings: Default::default(),
                buffer_device_address: false,
                allocation_sizes: Default::default(),
            })?;

            let swapchain = Swapchain::new(&ctx, vk::Extent2D { width, height }, None)?;
            let render_pass = create_render_pass(&ctx.device, swapchain.format)?;
            let depth = DepthBuffer::new(&ctx, &mut allocator, swapchain.extent)?;
            let framebuffers =
                create_framebuffers(&ctx.device, render_pass, &swapchain, &depth)?;

            let command_pool = ctx.device.create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
                    .queue_family_index(ctx.queue_family),
                None,
            )?;
            let command_buffers = ctx.device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(command_pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(FRAMES_IN_FLIGHT as u32),
            )?;

            // World pipelines target the HDR pass; the tonemap resolve and
            // UI target the swapchain pass.
            let hdr = HdrTarget::new(&ctx, &mut allocator, swapchain.extent)?;
            let bloom = BloomPass::new(&ctx, &mut allocator, hdr.view, hdr.extent)?;
            let exposure =
                ExposurePass::new(&ctx, &mut allocator, hdr.view, FRAMES_IN_FLIGHT)?;
            let tonemap = TonemapPass::new(&ctx, render_pass)?;
            tonemap.bind_input(&ctx.device, hdr.view, bloom.output());
            let shadow = ShadowPass::new(&ctx, &mut allocator, FRAMES_IN_FLIGHT)?;
            let chunks = ChunkRenderer::new(
                &ctx,
                &mut allocator,
                hdr.render_pass,
                hdr.water_pass,
                command_pool,
                shadow.descriptor_layout,
            )?;
            chunks.bind_water_inputs(&ctx.device, hdr.depth.view, hdr.scene_copy_view);
            let entity = EntityRenderer::new(&ctx, &mut allocator, hdr.render_pass)?;
            let outline = OutlineRenderer::new(&ctx, &mut allocator, hdr.render_pass)?;
            let sky = SkyPass::new(&ctx, hdr.render_pass)?;
            let clouds_layer = CloudLayer::new(&ctx, &mut allocator, hdr.render_pass)?;
            let far = FarRenderer::new(&ctx, hdr.render_pass)?;
            let ui =
                UiRenderer::new(&ctx, &mut allocator, render_pass, command_pool, FRAMES_IN_FLIGHT)?;

            let image_available = (0..FRAMES_IN_FLIGHT)
                .map(|_| ctx.device.create_semaphore(&Default::default(), None))
                .collect::<Result<Vec<_>, _>>()?;
            let render_finished = (0..swapchain.images.len())
                .map(|_| ctx.device.create_semaphore(&Default::default(), None))
                .collect::<Result<Vec<_>, _>>()?;
            let in_flight = (0..FRAMES_IN_FLIGHT)
                .map(|_| {
                    ctx.device.create_fence(
                        &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
                        None,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;

            Ok(Self {
                ctx,
                allocator: Some(allocator),
                swapchain,
                render_pass,
                depth,
                framebuffers,
                hdr,
                tonemap,
                resolution_scale: 1.0,
                scale_dirty: false,
                chunks,
                entity,
                outline,
                sky,
                clouds_layer,
                bloom,
                exposure,
                shadow,
                far,
                ui,
                command_pool,
                command_buffers,
                image_available,
                render_finished,
                in_flight,
                frame: 0,
                pending_extent: None,
                chunks_drawn: 0,
            })
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.pending_extent = Some(vk::Extent2D { width, height });
    }

    /// Sets the world render scale (UI stays native); applies next frame.
    pub fn set_resolution_scale(&mut self, scale: f32) {
        let scale = scale.clamp(0.25, 2.0);
        if (scale - self.resolution_scale).abs() > 1e-3 {
            self.resolution_scale = scale;
            self.scale_dirty = true;
        }
    }

    /// The HDR target extent for the current window size and scale.
    fn scaled_extent(&self) -> vk::Extent2D {
        vk::Extent2D {
            width: ((self.swapchain.extent.width as f32 * self.resolution_scale) as u32).max(1),
            height: ((self.swapchain.extent.height as f32 * self.resolution_scale) as u32).max(1),
        }
    }

    /// Uploads a section mesh at `pos`, replacing any previous one there.
    /// An empty mesh removes the chunk.
        /// Uploads (or replaces) one far-terrain tile.
    pub fn set_far_tile(&mut self, key: (i32, i32), tile: &FarTile) -> Result<()> {
        unsafe {
            let allocator = self.allocator.as_mut().expect("allocator alive");
            self.far.set_tile(&self.ctx, allocator, self.frame, key, tile)
        }
    }

    /// Drops a far tile that left the ring.
    pub fn remove_far_tile(&mut self, key: (i32, i32)) {
        self.far.remove_tile(self.frame, key);
    }

    pub fn set_chunk(&mut self, pos: SectionPos, meshes: &SectionMeshes) -> Result<()> {
        unsafe {
            let allocator = self.allocator.as_mut().expect("allocator alive");
            self.chunks.set_chunk(&self.ctx, allocator, pos, meshes, self.frame)
        }
    }

    /// Removes the chunk mesh at `pos`, if present.
    pub fn remove_chunk(&mut self, pos: SectionPos) {
        self.chunks.remove_chunk(pos, self.frame);
    }

    /// Removes every chunk mesh (leaving a world for the title screen).
    pub fn clear_chunks(&mut self) {
        self.chunks.clear_chunks(self.frame);
    }

    pub fn stats(&self) -> RenderStats {
        RenderStats {
            chunks_resident: self.chunks.chunk_count(),
            chunks_drawn: self.chunks_drawn,
        }
    }

    /// Renders one frame and presents it.
    pub fn draw(&mut self, camera: &FrameCamera) -> Result<()> {
        unsafe {
            let slot = (self.frame % FRAMES_IN_FLIGHT as u64) as usize;
            let fence = self.in_flight[slot];
            self.ctx.device.wait_for_fences(&[fence], true, u64::MAX)?;

            // The fence wait proves frame `frame - FRAMES_IN_FLIGHT` is done,
            // so buffers it referenced can be freed now.
            let allocator = self.allocator.as_mut().expect("allocator alive");
            self.chunks.collect_garbage(&self.ctx.device, allocator, self.frame);
            self.far.collect_garbage(&self.ctx.device, allocator, self.frame);
            // This slot's previous frame is done: its luminance grid is
            // readable, so the eye adapts now.
            let frame_exposure = self.exposure.adapt(slot, camera.time);

            if self.pending_extent.is_some() {
                self.recreate_swapchain()?;
            }
            if self.scale_dirty {
                self.scale_dirty = false;
                self.ctx.device.device_wait_idle()?;
                let extent = self.scaled_extent();
                let allocator = self.allocator.as_mut().expect("allocator alive");
                self.hdr.recreate(&self.ctx, allocator, extent)?;
                self.bloom.recreate(&self.ctx, allocator, self.hdr.view, extent)?;
                self.exposure.bind_input(&self.ctx.device, self.hdr.view);
                self.tonemap
                    .bind_input(&self.ctx.device, self.hdr.view, self.bloom.output());
                self.chunks.bind_water_inputs(&self.ctx.device, self.hdr.depth.view, self.hdr.scene_copy_view);
            }
            let device = &self.ctx.device;

            let acquire_sem = self.image_available[slot];
            let image_index = match self.swapchain.acquire(acquire_sem) {
                Ok(index) => index,
                Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                    self.pending_extent = Some(self.swapchain.extent);
                    return Ok(()); // recreate on the next draw
                }
                Err(e) => return Err(e).context("acquiring swapchain image"),
            };

            device.reset_fences(&[fence])?;

            let cmd = self.command_buffers[slot];
            device.reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty())?;
            device.begin_command_buffer(
                cmd,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;

            // Pass 0: the sun's view — three shadow cascades. Always run
            // (a cleared map keeps the layout valid when inactive).
            self.shadow.update(
                slot,
                camera.sun,
                camera.position,
                camera.shadows,
            );
            for cascade in 0..3 {
                self.shadow.begin(device, cmd, cascade);
                if self.shadow.active() {
                    self.chunks.record_shadow(
                        device,
                        cmd,
                        self.shadow.pipeline_layout,
                        self.shadow.cascade(cascade),
                        camera.position,
                    );
                }
                device.cmd_end_render_pass(cmd);
            }

            // Pass 1: the world, into the HDR target at the render scale.
            let world_extent = self.hdr.extent;
            device.cmd_set_viewport(
                cmd,
                0,
                &[vk::Viewport::default()
                    .width(world_extent.width as f32)
                    .height(world_extent.height as f32)
                    .max_depth(1.0)],
            );
            device.cmd_set_scissor(cmd, 0, &[world_extent.into()]);
            let clears = [
                vk::ClearValue {
                    color: vk::ClearColorValue { float32: camera.sky_color },
                },
                vk::ClearValue {
                    depth_stencil: vk::ClearDepthStencilValue { depth: 1.0, stencil: 0 },
                },
            ];
            device.cmd_begin_render_pass(
                cmd,
                &vk::RenderPassBeginInfo::default()
                    .render_pass(self.hdr.render_pass)
                    .framebuffer(self.hdr.framebuffer)
                    .render_area(world_extent.into())
                    .clear_values(&clears),
                vk::SubpassContents::INLINE,
            );
            let fog = Vec4::from_array(camera.sky_color).truncate().extend(camera.fog_distance);
            self.chunks_drawn = self.chunks.record(
                device,
                cmd,
                camera.view_proj,
                camera.position,
                camera.sun,
                camera.time,
                fog,
                self.shadow.descriptor_sets[slot],
            );
            // Far terrain ring: after the chunks, depth keeps detail on top.
            if camera.far_terrain {
                let daylight = camera.sun.truncate().length();
                self.far.record(
                    device,
                    cmd,
                    camera.view_proj,
                    camera.position,
                    fog,
                    camera.sun.w + (1.0 - camera.sun.w) * daylight * 0.8,
                );
            }
            self.entity
                .record(device, cmd, camera.view_proj, camera.position, &camera.entities);
            if let Some(block) = camera.highlight {
                self.outline
                    .record(device, cmd, camera.view_proj, camera.position, block);
            }
            // The sky dome shades only pixels no geometry wrote. The
            // scalars ride in the w slots; see SkyPush.
            let horizon = Vec4::from_array(camera.sky_color)
                .truncate()
                .extend(camera.sky_angle);
            self.sky.record(
                device,
                cmd,
                camera.view_proj,
                Vec4::from_array(camera.sky_sun),
                horizon,
                Vec4::from_array(camera.sky_away),
                Vec4::from_array(camera.sky_zenith),
            );
            if camera.clouds {
                self.clouds_layer.record(
                    device,
                    cmd,
                    camera.view_proj,
                    camera.position,
                    camera.time,
                    Vec4::from_array(camera.cloud_color),
                );
            }
            device.cmd_end_render_pass(cmd);

            // Between passes: snapshot the opaque color so water can
            // reflect the scene while blending into the original.
            let color_range = vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .level_count(1)
                .layer_count(1);
            let to_transfer = [
                vk::ImageMemoryBarrier::default()
                    .image(self.hdr.image)
                    .subresource_range(color_range)
                    .old_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                    .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                    .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
                    .dst_access_mask(vk::AccessFlags::TRANSFER_READ),
                vk::ImageMemoryBarrier::default()
                    .image(self.hdr.scene_copy)
                    .subresource_range(color_range)
                    .old_layout(vk::ImageLayout::UNDEFINED)
                    .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .src_access_mask(vk::AccessFlags::empty())
                    .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE),
            ];
            device.cmd_pipeline_barrier(
                cmd,
                // Prior-frame water reads of the old snapshot end here too.
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &to_transfer,
            );
            device.cmd_copy_image(
                cmd,
                self.hdr.image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                self.hdr.scene_copy,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[vk::ImageCopy::default()
                    .src_subresource(
                        vk::ImageSubresourceLayers::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .layer_count(1),
                    )
                    .dst_subresource(
                        vk::ImageSubresourceLayers::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .layer_count(1),
                    )
                    .extent(vk::Extent3D {
                        width: world_extent.width,
                        height: world_extent.height,
                        depth: 1,
                    })],
            );
            let from_transfer = [
                vk::ImageMemoryBarrier::default()
                    .image(self.hdr.image)
                    .subresource_range(color_range)
                    .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                    .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                    .src_access_mask(vk::AccessFlags::TRANSFER_READ)
                    .dst_access_mask(
                        vk::AccessFlags::COLOR_ATTACHMENT_READ
                            | vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
                    ),
                vk::ImageMemoryBarrier::default()
                    .image(self.hdr.scene_copy)
                    .subresource_range(color_range)
                    .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .dst_access_mask(vk::AccessFlags::SHADER_READ),
            ];
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &from_transfer,
            );

            // Pass 1b: water, blended over the opaques, sampling their
            // depth (absorption, shore fade, in-shader occlusion).
            device.cmd_begin_render_pass(
                cmd,
                &vk::RenderPassBeginInfo::default()
                    .render_pass(self.hdr.water_pass)
                    .framebuffer(self.hdr.water_framebuffer)
                    .render_area(world_extent.into()),
                vk::SubpassContents::INLINE,
            );
            self.chunks.record_water(
                device,
                cmd,
                camera.view_proj,
                camera.position,
                camera.sun,
                Vec4::from_array(camera.sky_color),
                camera.time,
                camera.fog_distance,
                slot,
                world_extent,
                camera.water_reflections,
            );
            device.cmd_end_render_pass(cmd);

            // Pass 1c: luminance measurement (for the next frames'
            // exposure), then the bloom pyramid.
            self.exposure.record(device, cmd, slot);
            self.bloom.record(device, cmd);

            // Pass 2: tonemap resolve + UI, at native resolution.
            let extent = self.swapchain.extent;
            device.cmd_set_viewport(
                cmd,
                0,
                &[vk::Viewport::default()
                    .width(extent.width as f32)
                    .height(extent.height as f32)
                    .max_depth(1.0)],
            );
            device.cmd_set_scissor(cmd, 0, &[extent.into()]);
            device.cmd_begin_render_pass(
                cmd,
                &vk::RenderPassBeginInfo::default()
                    .render_pass(self.render_pass)
                    .framebuffer(self.framebuffers[image_index as usize])
                    .render_area(extent.into())
                    .clear_values(&clears),
                vk::SubpassContents::INLINE,
            );
            self.tonemap.record(device, cmd, frame_exposure);
            if !camera.hud.is_empty() || !camera.ui_quads.is_empty() || !camera.ui_texts.is_empty()
            {
                let mut texts = Vec::with_capacity(camera.ui_texts.len() + 1);
                if !camera.hud.is_empty() {
                    let scale = camera.hud_scale.max(0.5);
                    texts.push(ui::UiText {
                        text: camera.hud.clone(),
                        x: 6.0 * scale,
                        y: 6.0 * scale,
                        scale,
                    });
                }
                texts.extend(camera.ui_texts.iter().cloned());
                self.ui
                    .record(device, cmd, slot, extent, &texts, &camera.ui_quads);
            }

            device.cmd_end_render_pass(cmd);
            device.end_command_buffer(cmd)?;

            let render_sem = self.render_finished[image_index as usize];
            let wait_stage = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
            let submit = vk::SubmitInfo::default()
                .wait_semaphores(std::slice::from_ref(&acquire_sem))
                .wait_dst_stage_mask(&wait_stage)
                .command_buffers(std::slice::from_ref(&cmd))
                .signal_semaphores(std::slice::from_ref(&render_sem));
            device.queue_submit(self.ctx.queue, &[submit], fence)?;

            match self.swapchain.present(self.ctx.queue, image_index, render_sem) {
                Ok(suboptimal) if suboptimal => {
                    self.pending_extent = Some(self.swapchain.extent);
                }
                Ok(_) => {}
                Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                    self.pending_extent = Some(self.swapchain.extent);
                }
                Err(e) => return Err(e).context("presenting swapchain image"),
            }

            self.frame += 1;
            Ok(())
        }
    }

    unsafe fn recreate_swapchain(&mut self) -> Result<()> {
        unsafe {
            let extent = self.pending_extent.take().expect("no pending extent");
            if extent.width == 0 || extent.height == 0 {
                // Minimized; keep the old swapchain until we have an area again.
                return Ok(());
            }
            let device = &self.ctx.device;
            device.device_wait_idle()?;
            let allocator = self.allocator.as_mut().expect("allocator alive");

            for fb in self.framebuffers.drain(..) {
                device.destroy_framebuffer(fb, None);
            }
            self.depth.destroy(device, allocator);

            let new_swapchain = Swapchain::new(&self.ctx, extent, Some(&self.swapchain))?;
            let old = std::mem::replace(&mut self.swapchain, new_swapchain);
            old.destroy(&self.ctx);

            // Swapchain image count can change; rebuild the per-image semaphores.
            for sem in self.render_finished.drain(..) {
                device.destroy_semaphore(sem, None);
            }
            self.render_finished = (0..self.swapchain.images.len())
                .map(|_| device.create_semaphore(&Default::default(), None))
                .collect::<Result<Vec<_>, _>>()?;

            self.depth = DepthBuffer::new(&self.ctx, allocator, self.swapchain.extent)?;
            self.framebuffers =
                create_framebuffers(device, self.render_pass, &self.swapchain, &self.depth)?;
            // The world target tracks the window times the render scale.
            let scaled = self.scaled_extent();
            let allocator = self.allocator.as_mut().expect("allocator alive");
            self.hdr.recreate(&self.ctx, allocator, scaled)?;
            self.bloom.recreate(&self.ctx, allocator, self.hdr.view, scaled)?;
            self.exposure.bind_input(&self.ctx.device, self.hdr.view);
            self.tonemap
                .bind_input(&self.ctx.device, self.hdr.view, self.bloom.output());
            self.chunks.bind_water_inputs(&self.ctx.device, self.hdr.depth.view, self.hdr.scene_copy_view);
            self.scale_dirty = false;
            Ok(())
        }
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe {
            let device = &self.ctx.device;
            let _ = device.device_wait_idle();
            let mut allocator = self.allocator.take().expect("allocator alive");

            self.chunks.destroy(device, &mut allocator);
            self.entity.destroy(device, &mut allocator);
            self.outline.destroy(device, &mut allocator);
            self.sky.destroy(device);
            self.clouds_layer.destroy(device, &mut allocator);
            self.far.destroy(device, &mut allocator);
            self.ui.destroy(device, &mut allocator);
            self.bloom.destroy(device, &mut allocator);
            self.exposure.destroy(device, &mut allocator);
            self.shadow.destroy(device, &mut allocator);
            self.tonemap.destroy(device);
            self.hdr.destroy(device, &mut allocator);
            for &sem in self.image_available.iter().chain(&self.render_finished) {
                device.destroy_semaphore(sem, None);
            }
            for &fence in &self.in_flight {
                device.destroy_fence(fence, None);
            }
            device.destroy_command_pool(self.command_pool, None);
            for &fb in &self.framebuffers {
                device.destroy_framebuffer(fb, None);
            }
            self.depth.destroy(device, &mut allocator);
            device.destroy_render_pass(self.render_pass, None);
            self.swapchain.destroy(&self.ctx);
            // The allocator must be dropped before the device it allocates from.
            drop(allocator);
            // VulkanContext's own Drop tears down device, surface, instance.
        }
    }
}

unsafe fn create_render_pass(device: &ash::Device, format: vk::Format) -> Result<vk::RenderPass> {
    let attachments = [
        vk::AttachmentDescription::default()
            .format(format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::PRESENT_SRC_KHR),
        vk::AttachmentDescription::default()
            .format(depth::DEPTH_FORMAT)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::DONT_CARE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL),
    ];
    let color_ref = [vk::AttachmentReference::default()
        .attachment(0)
        .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
    let depth_ref = vk::AttachmentReference::default()
        .attachment(1)
        .layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
    let subpass = vk::SubpassDescription::default()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .color_attachments(&color_ref)
        .depth_stencil_attachment(&depth_ref);
    let stages = vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
        | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS
        | vk::PipelineStageFlags::LATE_FRAGMENT_TESTS;
    let dependency = vk::SubpassDependency::default()
        .src_subpass(vk::SUBPASS_EXTERNAL)
        .dst_subpass(0)
        .src_stage_mask(stages)
        .src_access_mask(vk::AccessFlags::empty())
        .dst_stage_mask(stages)
        .dst_access_mask(
            vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
        );

    let render_pass = unsafe {
        device.create_render_pass(
            &vk::RenderPassCreateInfo::default()
                .attachments(&attachments)
                .subpasses(std::slice::from_ref(&subpass))
                .dependencies(std::slice::from_ref(&dependency)),
            None,
        )?
    };
    Ok(render_pass)
}

unsafe fn create_framebuffers(
    device: &ash::Device,
    render_pass: vk::RenderPass,
    swapchain: &Swapchain,
    depth: &DepthBuffer,
) -> Result<Vec<vk::Framebuffer>> {
    swapchain
        .image_views
        .iter()
        .map(|&view| {
            let attachments = [view, depth.view];
            let fb = unsafe {
                device.create_framebuffer(
                    &vk::FramebufferCreateInfo::default()
                        .render_pass(render_pass)
                        .attachments(&attachments)
                        .width(swapchain.extent.width)
                        .height(swapchain.extent.height)
                        .layers(1),
                    None,
                )?
            };
            Ok(fb)
        })
        .collect()
}
