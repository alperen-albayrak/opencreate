//! The offscreen HDR world target (graphics roadmap stage A2): the world
//! renders into a small-float color image at a scalable resolution, then
//! a fullscreen pass tonemaps it into the sRGB swapchain. UI draws after
//! the resolve, at native resolution.

use anyhow::Result;
use ash::vk;
use gpu_allocator::MemoryLocation;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme, Allocator};

use crate::context::VulkanContext;
use crate::depth::{self, DepthBuffer};

/// 32-bit HDR: plenty of range for sun/sky values, half the bandwidth
/// of RGBA16F, universally supported as a render target.
pub const HDR_FORMAT: vk::Format = vk::Format::B10G11R11_UFLOAT_PACK32;

const TONEMAP_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/tonemap.spv"));

/// The world's render target: HDR color + depth at `extent` (the
/// swapchain size times the resolution scale).
pub struct HdrTarget {
    pub extent: vk::Extent2D,
    /// Opaque world pass (chunk/entity/outline pipelines). Depth ends
    /// SHADER_READ_ONLY so the water pass can sample it.
    pub render_pass: vk::RenderPass,
    /// Water pass: loads the color, no depth attachment (water samples
    /// the opaque depth and depth-tests in the shader). Color ends in
    /// SHADER_READ_ONLY_OPTIMAL for the tonemap pass.
    pub water_pass: vk::RenderPass,
    image: vk::Image,
    allocation: Option<Allocation>,
    pub view: vk::ImageView,
    pub depth: DepthBuffer,
    pub framebuffer: vk::Framebuffer,
    pub water_framebuffer: vk::Framebuffer,
}

impl HdrTarget {
    pub unsafe fn new(
        ctx: &VulkanContext,
        allocator: &mut Allocator,
        extent: vk::Extent2D,
    ) -> Result<Self> {
        unsafe {
            let render_pass = create_world_pass(&ctx.device)?;
            let water_pass = create_water_pass(&ctx.device)?;
            let (image, allocation, view, depth, framebuffer, water_framebuffer) =
                create_images(ctx, allocator, render_pass, water_pass, extent)?;
            Ok(Self {
                extent,
                render_pass,
                water_pass,
                image,
                allocation: Some(allocation),
                view,
                depth,
                framebuffer,
                water_framebuffer,
            })
        }
    }

    /// Rebuilds the images at a new extent (render pass survives).
    /// Caller must have waited for the device to be idle.
    pub unsafe fn recreate(
        &mut self,
        ctx: &VulkanContext,
        allocator: &mut Allocator,
        extent: vk::Extent2D,
    ) -> Result<()> {
        unsafe {
            self.destroy_images(&ctx.device, allocator);
            let (image, allocation, view, depth, framebuffer, water_framebuffer) =
                create_images(ctx, allocator, self.render_pass, self.water_pass, extent)?;
            self.extent = extent;
            self.image = image;
            self.allocation = Some(allocation);
            self.view = view;
            self.depth = depth;
            self.framebuffer = framebuffer;
            self.water_framebuffer = water_framebuffer;
            Ok(())
        }
    }

    unsafe fn destroy_images(&mut self, device: &ash::Device, allocator: &mut Allocator) {
        unsafe {
            device.destroy_framebuffer(self.water_framebuffer, None);
            device.destroy_framebuffer(self.framebuffer, None);
            self.depth.destroy(device, allocator);
            device.destroy_image_view(self.view, None);
            device.destroy_image(self.image, None);
            if let Some(allocation) = self.allocation.take() {
                let _ = allocator.free(allocation);
            }
        }
    }

    pub unsafe fn destroy(&mut self, device: &ash::Device, allocator: &mut Allocator) {
        unsafe {
            self.destroy_images(device, allocator);
            device.destroy_render_pass(self.water_pass, None);
            device.destroy_render_pass(self.render_pass, None);
        }
    }
}

unsafe fn create_images(
    ctx: &VulkanContext,
    allocator: &mut Allocator,
    render_pass: vk::RenderPass,
    water_pass: vk::RenderPass,
    extent: vk::Extent2D,
) -> Result<(
    vk::Image,
    Allocation,
    vk::ImageView,
    DepthBuffer,
    vk::Framebuffer,
    vk::Framebuffer,
)> {
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
                .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED)
                .initial_layout(vk::ImageLayout::UNDEFINED),
            None,
        )?;
        let requirements = ctx.device.get_image_memory_requirements(image);
        let allocation = allocator.allocate(&AllocationCreateDesc {
            name: "hdr color",
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
        let depth = DepthBuffer::new(ctx, allocator, extent)?;
        let attachments = [view, depth.view];
        let framebuffer = ctx.device.create_framebuffer(
            &vk::FramebufferCreateInfo::default()
                .render_pass(render_pass)
                .attachments(&attachments)
                .width(extent.width)
                .height(extent.height)
                .layers(1),
            None,
        )?;
        let water_attachments = [view];
        let water_framebuffer = ctx.device.create_framebuffer(
            &vk::FramebufferCreateInfo::default()
                .render_pass(water_pass)
                .attachments(&water_attachments)
                .width(extent.width)
                .height(extent.height)
                .layers(1),
            None,
        )?;
        Ok((image, allocation, view, depth, framebuffer, water_framebuffer))
    }
}

/// The opaque world pass. Color stays an attachment (the water pass
/// continues into it); depth ends shader-readable for the water pass.
unsafe fn create_world_pass(device: &ash::Device) -> Result<vk::RenderPass> {
    unsafe {
        let attachments = [
            vk::AttachmentDescription::default()
                .format(HDR_FORMAT)
                .samples(vk::SampleCountFlags::TYPE_1)
                .load_op(vk::AttachmentLoadOp::CLEAR)
                .store_op(vk::AttachmentStoreOp::STORE)
                .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
                .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
                .initial_layout(vk::ImageLayout::UNDEFINED)
                .final_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL),
            vk::AttachmentDescription::default()
                .format(depth::DEPTH_FORMAT)
                .samples(vk::SampleCountFlags::TYPE_1)
                .load_op(vk::AttachmentLoadOp::CLEAR)
                .store_op(vk::AttachmentStoreOp::STORE)
                .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
                .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
                .initial_layout(vk::ImageLayout::UNDEFINED)
                .final_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL),
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
        // Entry: with two frames in flight, the PREVIOUS frame's water
        // pass samples this depth image and the tonemap samples the color
        // image — both in FRAGMENT_SHADER. The clears here must wait for
        // those reads (write-after-read needs only the execution
        // dependency), or water flickers out whenever the GPU overlaps
        // frames. Exit: color must be visible to the tonemap pass's
        // fragment sampling.
        let dependencies = [
            vk::SubpassDependency::default()
                .src_subpass(vk::SUBPASS_EXTERNAL)
                .dst_subpass(0)
                .src_stage_mask(
                    vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                        | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS
                        | vk::PipelineStageFlags::LATE_FRAGMENT_TESTS
                        | vk::PipelineStageFlags::FRAGMENT_SHADER,
                )
                .src_access_mask(vk::AccessFlags::empty())
                .dst_stage_mask(
                    vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                        | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS
                        | vk::PipelineStageFlags::LATE_FRAGMENT_TESTS,
                )
                .dst_access_mask(
                    vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                        | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
                ),
            vk::SubpassDependency::default()
                .src_subpass(0)
                .dst_subpass(vk::SUBPASS_EXTERNAL)
                .src_stage_mask(
                    vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                        | vk::PipelineStageFlags::LATE_FRAGMENT_TESTS,
                )
                .src_access_mask(
                    vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                        | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
                )
                .dst_stage_mask(
                    vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                        | vk::PipelineStageFlags::FRAGMENT_SHADER,
                )
                .dst_access_mask(
                    vk::AccessFlags::COLOR_ATTACHMENT_READ
                        | vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                        | vk::AccessFlags::SHADER_READ,
                ),
        ];
        let render_pass = device.create_render_pass(
            &vk::RenderPassCreateInfo::default()
                .attachments(&attachments)
                .subpasses(std::slice::from_ref(&subpass))
                .dependencies(&dependencies),
            None,
        )?;
        Ok(render_pass)
    }
}

/// The water pass: continues into the HDR color (no clear, no depth
/// attachment — water samples the opaque depth and tests in-shader) and
/// hands the color to the tonemap pass.
unsafe fn create_water_pass(device: &ash::Device) -> Result<vk::RenderPass> {
    unsafe {
        let attachments = [vk::AttachmentDescription::default()
            .format(HDR_FORMAT)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::LOAD)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .final_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let color_ref = [vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
        let subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&color_ref);
        let dependencies = [
            vk::SubpassDependency::default()
                .src_subpass(vk::SUBPASS_EXTERNAL)
                .dst_subpass(0)
                .src_stage_mask(
                    vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                        | vk::PipelineStageFlags::LATE_FRAGMENT_TESTS,
                )
                .src_access_mask(
                    vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                        | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
                )
                .dst_stage_mask(
                    vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                        | vk::PipelineStageFlags::FRAGMENT_SHADER,
                )
                .dst_access_mask(
                    vk::AccessFlags::COLOR_ATTACHMENT_READ
                        | vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                        | vk::AccessFlags::SHADER_READ,
                ),
            vk::SubpassDependency::default()
                .src_subpass(0)
                .dst_subpass(vk::SUBPASS_EXTERNAL)
                .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
                .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags::FRAGMENT_SHADER)
                .dst_access_mask(vk::AccessFlags::SHADER_READ),
        ];
        let render_pass = device.create_render_pass(
            &vk::RenderPassCreateInfo::default()
                .attachments(&attachments)
                .subpasses(std::slice::from_ref(&subpass))
                .dependencies(&dependencies),
            None,
        )?;
        Ok(render_pass)
    }
}

/// The fullscreen tonemap resolve, drawn first in the swapchain pass.
pub struct TonemapPass {
    descriptor_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    descriptor_set: vk::DescriptorSet,
    sampler: vk::Sampler,
    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
}

impl TonemapPass {
    pub unsafe fn new(ctx: &VulkanContext, swapchain_pass: vk::RenderPass) -> Result<Self> {
        unsafe {
            let device = &ctx.device;
            let bindings = [
                vk::DescriptorSetLayoutBinding::default()
                    .binding(0)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT),
                vk::DescriptorSetLayoutBinding::default()
                    .binding(1)
                    .descriptor_type(vk::DescriptorType::SAMPLER)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT),
                vk::DescriptorSetLayoutBinding::default()
                    .binding(2)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            ];
            let descriptor_layout = device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )?;
            let pool_sizes = [
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(2),
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
            // Linear filtering so downscaled rendering upsamples smoothly.
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
                .size(16);
            let pipeline_layout = device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(std::slice::from_ref(&descriptor_layout))
                    .push_constant_ranges(std::slice::from_ref(&push_range)),
                None,
            )?;
            let pipeline = create_pipeline(device, swapchain_pass, pipeline_layout)?;
            Ok(Self {
                descriptor_layout,
                descriptor_pool,
                descriptor_set,
                sampler,
                pipeline_layout,
                pipeline,
            })
        }
    }

    /// Points the resolve at the (re)created HDR image and bloom pyramid.
    /// Call while the device is idle (target recreation already requires
    /// that).
    pub unsafe fn bind_input(&self, device: &ash::Device, view: vk::ImageView, bloom: vk::ImageView) {
        unsafe {
            let image_info = [vk::DescriptorImageInfo::default()
                .image_view(view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            let sampler_info = [vk::DescriptorImageInfo::default().sampler(self.sampler)];
            let bloom_info = [vk::DescriptorImageInfo::default()
                .image_view(bloom)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            let writes = [
                vk::WriteDescriptorSet::default()
                    .dst_set(self.descriptor_set)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .image_info(&image_info),
                vk::WriteDescriptorSet::default()
                    .dst_set(self.descriptor_set)
                    .dst_binding(1)
                    .descriptor_type(vk::DescriptorType::SAMPLER)
                    .image_info(&sampler_info),
                vk::WriteDescriptorSet::default()
                    .dst_set(self.descriptor_set)
                    .dst_binding(2)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .image_info(&bloom_info),
            ];
            device.update_descriptor_sets(&writes, &[]);
        }
    }

    /// Records the fullscreen resolve. Must run inside the swapchain pass
    /// with viewport/scissor set to the native extent.
    pub unsafe fn record(&self, device: &ash::Device, cmd: vk::CommandBuffer, exposure: f32) {
        unsafe {
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout,
                0,
                &[self.descriptor_set],
                &[],
            );
            let push = [exposure, 0.0, 0.0, 0.0];
            device.cmd_push_constants(
                cmd,
                self.pipeline_layout,
                vk::ShaderStageFlags::FRAGMENT,
                0,
                crate::chunk_renderer::as_bytes(&push),
            );
            device.cmd_draw(cmd, 3, 1, 0, 0);
        }
    }

    pub unsafe fn destroy(&mut self, device: &ash::Device) {
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            device.destroy_sampler(self.sampler, None);
            device.destroy_descriptor_pool(self.descriptor_pool, None);
            device.destroy_descriptor_set_layout(self.descriptor_layout, None);
        }
    }
}

unsafe fn create_pipeline(
    device: &ash::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> Result<vk::Pipeline> {
    unsafe {
        let code = ash::util::read_spv(&mut std::io::Cursor::new(TONEMAP_SPV))?;
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
        // No depth test: it covers the screen and writes nothing.
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
