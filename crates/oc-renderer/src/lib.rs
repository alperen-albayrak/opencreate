//! The OpenCreate engine: Vulkan rendering via `ash` (ARCHITECTURE.md §4).
//!
//! Vulkan 1.2 baseline, constrained to what MoltenVK supports so macOS works
//! through the Vulkan-on-Metal translation layer. The renderer never sees game
//! logic; it consumes meshes and transforms.

mod chunk_renderer;
mod context;
mod depth;
mod mesh;
mod swapchain;
mod texture;

use anyhow::{Context as _, Result};
use ash::vk;
use glam::{DVec3, Mat4};
use gpu_allocator::vulkan::{Allocator, AllocatorCreateDesc};
use oc_world::Section;
use raw_window_handle::{RawDisplayHandle, RawWindowHandle};

use chunk_renderer::ChunkRenderer;
use context::VulkanContext;
use depth::DepthBuffer;
use swapchain::Swapchain;

/// Number of frames the CPU may record ahead of the GPU.
const FRAMES_IN_FLIGHT: usize = 2;

/// Per-frame camera state, camera-relative (§3): `view_proj` contains no
/// translation; world-space translation happens in f64 against `position`.
pub struct FrameCamera {
    pub view_proj: Mat4,
    pub position: DVec3,
}

/// Owns the Vulkan device, swapchain and per-frame state, and draws frames.
pub struct Renderer {
    ctx: VulkanContext,
    allocator: Option<Allocator>,
    swapchain: Swapchain,
    render_pass: vk::RenderPass,
    depth: DepthBuffer,
    framebuffers: Vec<vk::Framebuffer>,
    chunks: ChunkRenderer,
    command_pool: vk::CommandPool,
    command_buffers: Vec<vk::CommandBuffer>,
    /// Signalled when the acquired image is ready to be rendered to. Per frame in flight.
    image_available: Vec<vk::Semaphore>,
    /// Signalled when rendering to an image is done. Per swapchain image,
    /// because presentation waits on it after the frame's fence is reused.
    render_finished: Vec<vk::Semaphore>,
    in_flight: Vec<vk::Fence>,
    frame: usize,
    /// Pending window size to recreate the swapchain at, set on resize.
    pending_extent: Option<vk::Extent2D>,
    pub clear_color: [f32; 4],
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

            let chunks = ChunkRenderer::new(&ctx, &mut allocator, render_pass, command_pool)?;

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
                chunks,
                command_pool,
                command_buffers,
                image_available,
                render_finished,
                in_flight,
                frame: 0,
                pending_extent: None,
                // Sky blue; replaced by the real sky once there is a world.
                clear_color: [0.47, 0.71, 0.99, 1.0],
            })
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.pending_extent = Some(vk::Extent2D { width, height });
    }

    /// Meshes and uploads a section at `origin` (its minimum corner, world space).
    pub fn set_test_chunk(&mut self, section: &Section, origin: DVec3) -> Result<()> {
        unsafe {
            let allocator = self.allocator.as_mut().expect("allocator alive");
            self.chunks.set_chunk(&self.ctx, allocator, section, origin)
        }
    }

    /// Renders one frame and presents it.
    pub fn draw(&mut self, camera: &FrameCamera) -> Result<()> {
        unsafe {
            let fence = self.in_flight[self.frame];
            self.ctx.device.wait_for_fences(&[fence], true, u64::MAX)?;

            if self.pending_extent.is_some() {
                self.recreate_swapchain()?;
            }
            let device = &self.ctx.device;

            let acquire_sem = self.image_available[self.frame];
            let image_index = match self.swapchain.acquire(acquire_sem) {
                Ok(index) => index,
                Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                    self.pending_extent = Some(self.swapchain.extent);
                    return Ok(()); // recreate on the next draw
                }
                Err(e) => return Err(e).context("acquiring swapchain image"),
            };

            device.reset_fences(&[fence])?;

            let cmd = self.command_buffers[self.frame];
            device.reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty())?;
            device.begin_command_buffer(
                cmd,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;

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

            let clears = [
                vk::ClearValue {
                    color: vk::ClearColorValue { float32: self.clear_color },
                },
                vk::ClearValue {
                    depth_stencil: vk::ClearDepthStencilValue { depth: 1.0, stencil: 0 },
                },
            ];
            device.cmd_begin_render_pass(
                cmd,
                &vk::RenderPassBeginInfo::default()
                    .render_pass(self.render_pass)
                    .framebuffer(self.framebuffers[image_index as usize])
                    .render_area(extent.into())
                    .clear_values(&clears),
                vk::SubpassContents::INLINE,
            );

            self.chunks.record(device, cmd, camera.view_proj, camera.position);

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

            self.frame = (self.frame + 1) % FRAMES_IN_FLIGHT;
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
