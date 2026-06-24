//! Temporal anti-aliasing (graphics roadmap P5). A fullscreen resolve run after
//! the scene composites into the HDR target: it blends the current sub-pixel-
//! jittered frame with the reprojected history so voxel edges, per-texel normal
//! maps and specular highlights stop shimmering.
//!
//! Owns two HDR images: `resolved` (this frame's output — what exposure, bloom
//! and tonemap now read) and `hist` (the previous resolved frame, sampled by
//! the resolve and refreshed by a copy of `resolved` each frame). The
//! reprojection matrix and a validity flag ride in push constants; the camera
//! delta is folded into the matrix CPU-side (see `lib.rs`), so camera-relative
//! rendering reprojects correctly without a velocity buffer.

use anyhow::Result;
use ash::vk;
use glam::{Mat4, Vec4};
use gpu_allocator::MemoryLocation;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme, Allocator};

use crate::chunk_renderer::as_bytes;
use crate::context::VulkanContext;
use crate::hdr::HDR_FORMAT;

const TAA_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/taa.spv"));

/// Push constants for the resolve (80 bytes, ≤ the 128-byte guaranteed limit).
#[repr(C)]
#[derive(Clone, Copy)]
struct TaaPush {
    /// `VP_prev · translate(camera_delta) · inv(VP_cur)`: current NDC → prev NDC.
    reproj: Mat4,
    /// x: history valid (1 = blend, 0 = passthrough); y: feedback weight; zw: 0.
    params: Vec4,
}

pub struct TaaPass {
    render_pass: vk::RenderPass,
    extent: vk::Extent2D,
    resolved_image: vk::Image,
    resolved_allocation: Option<Allocation>,
    resolved_view: vk::ImageView,
    hist_image: vk::Image,
    hist_allocation: Option<Allocation>,
    hist_view: vk::ImageView,
    framebuffer: vk::Framebuffer,
    descriptor_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    descriptor_set: vk::DescriptorSet,
    sampler: vk::Sampler,
    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
}

impl TaaPass {
    pub unsafe fn new(
        ctx: &VulkanContext,
        allocator: &mut Allocator,
        command_pool: vk::CommandPool,
        extent: vk::Extent2D,
    ) -> Result<Self> {
        unsafe {
            let device = &ctx.device;
            let render_pass = create_render_pass(device)?;

            let bindings = [
                binding(0, vk::DescriptorType::SAMPLED_IMAGE),
                binding(1, vk::DescriptorType::SAMPLER),
                binding(2, vk::DescriptorType::SAMPLED_IMAGE),
                binding(3, vk::DescriptorType::SAMPLED_IMAGE),
            ];
            let descriptor_layout = device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )?;
            let pool_sizes = [
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(3),
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::SAMPLER)
                    .descriptor_count(1),
            ];
            let descriptor_pool = device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(1)
                    .pool_sizes(&pool_sizes),
                None,
            )?;
            let descriptor_set = device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(descriptor_pool)
                    .set_layouts(std::slice::from_ref(&descriptor_layout)),
            )?[0];
            // Linear: history is sampled at a reprojected sub-texel UV.
            let sampler = device.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::LINEAR)
                    .min_filter(vk::Filter::LINEAR)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
                None,
            )?;

            let push_range = vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::FRAGMENT)
                .size(size_of::<TaaPush>() as u32);
            let pipeline_layout = device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(std::slice::from_ref(&descriptor_layout))
                    .push_constant_ranges(std::slice::from_ref(&push_range)),
                None,
            )?;
            let pipeline = create_pipeline(device, render_pass, pipeline_layout)?;

            let (
                resolved_image,
                resolved_allocation,
                resolved_view,
                hist_image,
                hist_allocation,
                hist_view,
                framebuffer,
            ) = create_images(ctx, allocator, command_pool, render_pass, extent)?;

            Ok(Self {
                render_pass,
                extent,
                resolved_image,
                resolved_allocation: Some(resolved_allocation),
                resolved_view,
                hist_image,
                hist_allocation: Some(hist_allocation),
                hist_view,
                framebuffer,
                descriptor_layout,
                descriptor_pool,
                descriptor_set,
                sampler,
                pipeline_layout,
                pipeline,
            })
        }
    }

    /// The resolved (anti-aliased) HDR image — what exposure, bloom and tonemap
    /// sample instead of the raw scene HDR.
    pub fn resolved_view(&self) -> vk::ImageView {
        self.resolved_view
    }

    /// Points the resolve at the (re)created scene HDR + depth. Binds the stable
    /// history view + sampler too. Call after `new`/`resize`, device idle.
    pub unsafe fn bind_inputs(
        &self,
        device: &ash::Device,
        current: vk::ImageView,
        depth: vk::ImageView,
    ) {
        unsafe {
            let cur = [image_info(current)];
            let hist = [image_info(self.hist_view)];
            let dep = [image_info(depth)];
            let samp = [vk::DescriptorImageInfo::default().sampler(self.sampler)];
            let writes = [
                write(self.descriptor_set, 0, vk::DescriptorType::SAMPLED_IMAGE, &cur),
                write(self.descriptor_set, 1, vk::DescriptorType::SAMPLER, &samp),
                write(self.descriptor_set, 2, vk::DescriptorType::SAMPLED_IMAGE, &hist),
                write(self.descriptor_set, 3, vk::DescriptorType::SAMPLED_IMAGE, &dep),
            ];
            device.update_descriptor_sets(&writes, &[]);
        }
    }

    /// Records the resolve into `resolved`. Inherits the world-extent viewport
    /// the caller set for the preceding passes.
    pub unsafe fn record(
        &self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        reproj: Mat4,
        valid: bool,
        feedback: f32,
    ) {
        unsafe {
            device.cmd_begin_render_pass(
                cmd,
                &vk::RenderPassBeginInfo::default()
                    .render_pass(self.render_pass)
                    .framebuffer(self.framebuffer)
                    .render_area(self.extent.into()),
                vk::SubpassContents::INLINE,
            );
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout,
                0,
                &[self.descriptor_set],
                &[],
            );
            let push = TaaPush {
                reproj,
                params: Vec4::new(if valid { 1.0 } else { 0.0 }, feedback, 0.0, 0.0),
            };
            device.cmd_push_constants(
                cmd,
                self.pipeline_layout,
                vk::ShaderStageFlags::FRAGMENT,
                0,
                as_bytes(std::slice::from_ref(&push)),
            );
            device.cmd_draw(cmd, 3, 1, 0, 0);
            device.cmd_end_render_pass(cmd);
        }
    }

    /// Copies `resolved` → `hist` so next frame reprojects against it. Mirrors
    /// the scene-copy dance: SHADER_READ → TRANSFER → SHADER_READ on both, so
    /// `resolved` is back to a sampleable layout for exposure/bloom/tonemap.
    pub unsafe fn copy_to_history(&self, device: &ash::Device, cmd: vk::CommandBuffer) {
        unsafe {
            let range = vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .level_count(1)
                .layer_count(1);
            let to_transfer = [
                barrier(self.resolved_image, range)
                    .old_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                    .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                    .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
                    .dst_access_mask(vk::AccessFlags::TRANSFER_READ),
                barrier(self.hist_image, range)
                    .old_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                    .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .src_access_mask(vk::AccessFlags::SHADER_READ)
                    .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE),
            ];
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &to_transfer,
            );
            let layers = vk::ImageSubresourceLayers::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .layer_count(1);
            device.cmd_copy_image(
                cmd,
                self.resolved_image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                self.hist_image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[vk::ImageCopy::default()
                    .src_subresource(layers)
                    .dst_subresource(layers)
                    .extent(vk::Extent3D {
                        width: self.extent.width,
                        height: self.extent.height,
                        depth: 1,
                    })],
            );
            let from_transfer = [
                barrier(self.resolved_image, range)
                    .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                    .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                    .src_access_mask(vk::AccessFlags::TRANSFER_READ)
                    .dst_access_mask(vk::AccessFlags::SHADER_READ),
                barrier(self.hist_image, range)
                    .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .dst_access_mask(vk::AccessFlags::SHADER_READ),
            ];
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &from_transfer,
            );
        }
    }

    /// Rebuilds the images at a new extent (render pass + pipeline survive).
    /// Caller must wait for device idle, then `bind_inputs` again.
    pub unsafe fn resize(
        &mut self,
        ctx: &VulkanContext,
        allocator: &mut Allocator,
        command_pool: vk::CommandPool,
        extent: vk::Extent2D,
    ) -> Result<()> {
        unsafe {
            self.destroy_images(&ctx.device, allocator);
            let (ri, ra, rv, hi, ha, hv, fb) =
                create_images(ctx, allocator, command_pool, self.render_pass, extent)?;
            self.extent = extent;
            self.resolved_image = ri;
            self.resolved_allocation = Some(ra);
            self.resolved_view = rv;
            self.hist_image = hi;
            self.hist_allocation = Some(ha);
            self.hist_view = hv;
            self.framebuffer = fb;
            Ok(())
        }
    }

    unsafe fn destroy_images(&mut self, device: &ash::Device, allocator: &mut Allocator) {
        unsafe {
            device.destroy_framebuffer(self.framebuffer, None);
            device.destroy_image_view(self.resolved_view, None);
            device.destroy_image(self.resolved_image, None);
            if let Some(a) = self.resolved_allocation.take() {
                let _ = allocator.free(a);
            }
            device.destroy_image_view(self.hist_view, None);
            device.destroy_image(self.hist_image, None);
            if let Some(a) = self.hist_allocation.take() {
                let _ = allocator.free(a);
            }
        }
    }

    pub unsafe fn destroy(&mut self, device: &ash::Device, allocator: &mut Allocator) {
        unsafe {
            self.destroy_images(device, allocator);
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            device.destroy_sampler(self.sampler, None);
            device.destroy_descriptor_pool(self.descriptor_pool, None);
            device.destroy_descriptor_set_layout(self.descriptor_layout, None);
            device.destroy_render_pass(self.render_pass, None);
        }
    }
}

fn binding(slot: u32, ty: vk::DescriptorType) -> vk::DescriptorSetLayoutBinding<'static> {
    vk::DescriptorSetLayoutBinding::default()
        .binding(slot)
        .descriptor_type(ty)
        .descriptor_count(1)
        .stage_flags(vk::ShaderStageFlags::FRAGMENT)
}

fn image_info(view: vk::ImageView) -> vk::DescriptorImageInfo {
    vk::DescriptorImageInfo::default()
        .image_view(view)
        .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
}

fn write<'a>(
    set: vk::DescriptorSet,
    slot: u32,
    ty: vk::DescriptorType,
    info: &'a [vk::DescriptorImageInfo],
) -> vk::WriteDescriptorSet<'a> {
    vk::WriteDescriptorSet::default()
        .dst_set(set)
        .dst_binding(slot)
        .descriptor_type(ty)
        .image_info(info)
}

fn barrier(
    image: vk::Image,
    range: vk::ImageSubresourceRange,
) -> vk::ImageMemoryBarrier<'static> {
    vk::ImageMemoryBarrier::default().image(image).subresource_range(range)
}

/// One color attachment (the resolved image): contents overwritten every pixel
/// (DONT_CARE load), ends SHADER_READ_ONLY for exposure/bloom/tonemap + the copy.
unsafe fn create_render_pass(device: &ash::Device) -> Result<vk::RenderPass> {
    unsafe {
        let attachments = [vk::AttachmentDescription::default()
            .format(HDR_FORMAT)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::DONT_CARE)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let color_ref = [vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
        let subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&color_ref);
        let dependencies = [
            // Entry: prior-frame reads of `resolved` (exposure/bloom/tonemap +
            // the history copy) must finish before this frame overwrites it.
            vk::SubpassDependency::default()
                .src_subpass(vk::SUBPASS_EXTERNAL)
                .dst_subpass(0)
                .src_stage_mask(
                    vk::PipelineStageFlags::FRAGMENT_SHADER | vk::PipelineStageFlags::TRANSFER,
                )
                .src_access_mask(vk::AccessFlags::SHADER_READ | vk::AccessFlags::TRANSFER_READ)
                .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
                .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE),
            // Exit: the resolved color visible to sampling (exposure/bloom) and
            // the transfer copy to history.
            vk::SubpassDependency::default()
                .src_subpass(0)
                .dst_subpass(vk::SUBPASS_EXTERNAL)
                .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
                .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
                .dst_stage_mask(
                    vk::PipelineStageFlags::FRAGMENT_SHADER | vk::PipelineStageFlags::TRANSFER,
                )
                .dst_access_mask(vk::AccessFlags::SHADER_READ | vk::AccessFlags::TRANSFER_READ),
        ];
        Ok(device.create_render_pass(
            &vk::RenderPassCreateInfo::default()
                .attachments(&attachments)
                .subpasses(std::slice::from_ref(&subpass))
                .dependencies(&dependencies),
            None,
        )?)
    }
}

type Images = (
    vk::Image,
    Allocation,
    vk::ImageView,
    vk::Image,
    Allocation,
    vk::ImageView,
    vk::Framebuffer,
);

unsafe fn create_images(
    ctx: &VulkanContext,
    allocator: &mut Allocator,
    command_pool: vk::CommandPool,
    render_pass: vk::RenderPass,
    extent: vk::Extent2D,
) -> Result<Images> {
    unsafe {
        // resolved: rendered into (COLOR), sampled downstream (SAMPLED), copied
        // to history (TRANSFER_SRC).
        let (resolved_image, resolved_allocation, resolved_view) = image(
            ctx,
            allocator,
            extent,
            vk::ImageUsageFlags::COLOR_ATTACHMENT
                | vk::ImageUsageFlags::SAMPLED
                | vk::ImageUsageFlags::TRANSFER_SRC,
            "taa resolved",
        )?;
        // history: sampled by the resolve (SAMPLED), refilled by the copy
        // (TRANSFER_DST).
        let (hist_image, hist_allocation, hist_view) = image(
            ctx,
            allocator,
            extent,
            vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
            "taa history",
        )?;
        let attachments = [resolved_view];
        let framebuffer = ctx.device.create_framebuffer(
            &vk::FramebufferCreateInfo::default()
                .render_pass(render_pass)
                .attachments(&attachments)
                .width(extent.width)
                .height(extent.height)
                .layers(1),
            None,
        )?;
        // The history starts as garbage (the first frame's `valid=0` ignores it),
        // but its layout must be SHADER_READ_ONLY before that first sample.
        init_history_layout(ctx, command_pool, hist_image)?;
        Ok((
            resolved_image,
            resolved_allocation,
            resolved_view,
            hist_image,
            hist_allocation,
            hist_view,
            framebuffer,
        ))
    }
}

unsafe fn image(
    ctx: &VulkanContext,
    allocator: &mut Allocator,
    extent: vk::Extent2D,
    usage: vk::ImageUsageFlags,
    name: &str,
) -> Result<(vk::Image, Allocation, vk::ImageView)> {
    unsafe {
        let image = ctx.device.create_image(
            &vk::ImageCreateInfo::default()
                .image_type(vk::ImageType::TYPE_2D)
                .format(HDR_FORMAT)
                .extent(vk::Extent3D { width: extent.width, height: extent.height, depth: 1 })
                .mip_levels(1)
                .array_layers(1)
                .samples(vk::SampleCountFlags::TYPE_1)
                .tiling(vk::ImageTiling::OPTIMAL)
                .usage(usage)
                .initial_layout(vk::ImageLayout::UNDEFINED),
            None,
        )?;
        let requirements = ctx.device.get_image_memory_requirements(image);
        let allocation = allocator.allocate(&AllocationCreateDesc {
            name,
            requirements,
            location: MemoryLocation::GpuOnly,
            linear: false,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })?;
        ctx.device
            .bind_image_memory(image, allocation.memory(), allocation.offset())?;
        let view = ctx.device.create_image_view(
            &vk::ImageViewCreateInfo::default()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(HDR_FORMAT)
                .subresource_range(
                    vk::ImageSubresourceRange::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .level_count(1)
                        .layer_count(1),
                ),
            None,
        )?;
        Ok((image, allocation, view))
    }
}

/// One-time transition of the freshly-created history image UNDEFINED →
/// SHADER_READ_ONLY, so the first frame's resolve can sample it (its contents
/// are ignored that frame via the `valid=0` flag).
unsafe fn init_history_layout(
    ctx: &VulkanContext,
    command_pool: vk::CommandPool,
    image: vk::Image,
) -> Result<()> {
    unsafe {
        let device = &ctx.device;
        let cmd = device.allocate_command_buffers(
            &vk::CommandBufferAllocateInfo::default()
                .command_pool(command_pool)
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(1),
        )?[0];
        device.begin_command_buffer(
            cmd,
            &vk::CommandBufferBeginInfo::default()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
        )?;
        let to_read = barrier(
            image,
            vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .level_count(1)
                .layer_count(1),
        )
        .old_layout(vk::ImageLayout::UNDEFINED)
        .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
        .src_access_mask(vk::AccessFlags::empty())
        .dst_access_mask(vk::AccessFlags::SHADER_READ);
        device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::FRAGMENT_SHADER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[to_read],
        );
        device.end_command_buffer(cmd)?;
        let submit = vk::SubmitInfo::default().command_buffers(std::slice::from_ref(&cmd));
        device.queue_submit(ctx.queue, &[submit], vk::Fence::null())?;
        device.queue_wait_idle(ctx.queue)?;
        device.free_command_buffers(command_pool, &[cmd]);
        Ok(())
    }
}

unsafe fn create_pipeline(
    device: &ash::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> Result<vk::Pipeline> {
    unsafe {
        let code = ash::util::read_spv(&mut std::io::Cursor::new(TAA_SPV))?;
        let module = device
            .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&code), None)?;
        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(module)
                .name(c"vs_main"),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(module)
                .name(c"fs_main"),
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport_state = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default();
        let blend_attachment = vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA);
        let blend = vk::PipelineColorBlendStateCreateInfo::default()
            .attachments(std::slice::from_ref(&blend_attachment));
        let dynamic = vk::PipelineDynamicStateCreateInfo::default()
            .dynamic_states(&[vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR]);
        let info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport_state)
            .rasterization_state(&rasterization)
            .multisample_state(&multisample)
            .depth_stencil_state(&depth_stencil)
            .color_blend_state(&blend)
            .dynamic_state(&dynamic)
            .layout(layout)
            .render_pass(render_pass);
        let pipeline = device
            .create_graphics_pipelines(vk::PipelineCache::null(), &[info], None)
            .map_err(|(_, e)| e)?[0];
        device.destroy_shader_module(module, None);
        Ok(pipeline)
    }
}
