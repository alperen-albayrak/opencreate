//! Bloom (graphics roadmap stage E): HDR highlights bleed into a glow.
//!
//! A mip pyramid starting at half the HDR target's resolution: dual-Kawase
//! downsamples walk down the chain (the first one soft-thresholds so only
//! true HDR values — sun, glints, lamps — survive), then tent upsamples
//! blend additively back up. The tonemap pass samples mip 0 and adds it
//! over the scene.

use anyhow::Result;
use ash::vk;
use glam::Vec4;
use gpu_allocator::MemoryLocation;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme, Allocator};

use crate::chunk_renderer::as_bytes;
use crate::context::VulkanContext;
use crate::hdr::HDR_FORMAT;

const BLOOM_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/bloom.spv"));

/// Pyramid depth cap; fewer levels at small window sizes.
const MAX_LEVELS: u32 = 6;

pub struct BloomPass {
    /// Writes a mip from scratch (downsample).
    down_pass: vk::RenderPass,
    /// Loads a mip and blends into it (upsample).
    up_pass: vk::RenderPass,
    down_pipeline: vk::Pipeline,
    up_pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    descriptor_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    sampler: vk::Sampler,
    // Everything below is rebuilt when the HDR target resizes.
    image: vk::Image,
    allocation: Option<Allocation>,
    views: Vec<vk::ImageView>,
    framebuffers: Vec<vk::Framebuffer>,
    extents: Vec<vk::Extent2D>,
    /// [0] samples the scene; [1 + i] samples mip i.
    sets: Vec<vk::DescriptorSet>,
}

impl BloomPass {
    pub unsafe fn new(
        ctx: &VulkanContext,
        allocator: &mut Allocator,
        scene: vk::ImageView,
        extent: vk::Extent2D,
    ) -> Result<Self> {
        unsafe {
            let device = &ctx.device;
            let down_pass = create_pass(device, false)?;
            let up_pass = create_pass(device, true)?;

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
            ];
            let descriptor_layout = device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )?;
            let max_sets = MAX_LEVELS + 1;
            let pool_sizes = [
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(max_sets),
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::SAMPLER)
                    .descriptor_count(max_sets),
            ];
            let descriptor_pool = device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(max_sets)
                    .pool_sizes(&pool_sizes),
                None,
            )?;
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
                .size(size_of::<Vec4>() as u32);
            let pipeline_layout = device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(std::slice::from_ref(&descriptor_layout))
                    .push_constant_ranges(std::slice::from_ref(&push_range)),
                None,
            )?;
            let down_pipeline =
                create_pipeline(device, down_pass, pipeline_layout, c"fs_down", false)?;
            let up_pipeline = create_pipeline(device, up_pass, pipeline_layout, c"fs_up", true)?;

            let mut bloom = Self {
                down_pass,
                up_pass,
                down_pipeline,
                up_pipeline,
                pipeline_layout,
                descriptor_layout,
                descriptor_pool,
                sampler,
                image: vk::Image::null(),
                allocation: None,
                views: Vec::new(),
                framebuffers: Vec::new(),
                extents: Vec::new(),
                sets: Vec::new(),
            };
            bloom.recreate(ctx, allocator, scene, extent)?;
            Ok(bloom)
        }
    }

    /// The view the tonemap pass composites (mip 0 of the pyramid).
    pub fn output(&self) -> vk::ImageView {
        self.views[0]
    }

    /// Rebuilds the pyramid for a new HDR extent. Caller must have waited
    /// for the device to be idle.
    pub unsafe fn recreate(
        &mut self,
        ctx: &VulkanContext,
        allocator: &mut Allocator,
        scene: vk::ImageView,
        extent: vk::Extent2D,
    ) -> Result<()> {
        unsafe {
            let device = &ctx.device;
            self.destroy_images(device, allocator);

            // Mip 0 is half the scene; stop before any side drops below 8.
            let (mut w, mut h) = (extent.width.max(2) / 2, extent.height.max(2) / 2);
            let mut extents = Vec::new();
            while extents.len() < MAX_LEVELS as usize && w >= 8 && h >= 8 {
                extents.push(vk::Extent2D { width: w, height: h });
                w /= 2;
                h /= 2;
            }
            let levels = extents.len() as u32;
            self.extents = extents;

            let image = device.create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(HDR_FORMAT)
                    .extent(vk::Extent3D {
                        width: self.extents[0].width,
                        height: self.extents[0].height,
                        depth: 1,
                    })
                    .mip_levels(levels)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED)
                    .initial_layout(vk::ImageLayout::UNDEFINED),
                None,
            )?;
            let requirements = device.get_image_memory_requirements(image);
            let allocation = allocator.allocate(&AllocationCreateDesc {
                name: "bloom pyramid",
                requirements,
                location: MemoryLocation::GpuOnly,
                linear: false,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })?;
            device.bind_image_memory(image, allocation.memory(), allocation.offset())?;
            self.image = image;
            self.allocation = Some(allocation);

            for level in 0..levels {
                let view = device.create_image_view(
                    &vk::ImageViewCreateInfo::default()
                        .image(image)
                        .view_type(vk::ImageViewType::TYPE_2D)
                        .format(HDR_FORMAT)
                        .subresource_range(
                            vk::ImageSubresourceRange::default()
                                .aspect_mask(vk::ImageAspectFlags::COLOR)
                                .base_mip_level(level)
                                .level_count(1)
                                .layer_count(1),
                        ),
                    None,
                )?;
                self.views.push(view);
                // Compatible with both passes (load ops don't affect
                // render pass compatibility).
                self.framebuffers.push(device.create_framebuffer(
                    &vk::FramebufferCreateInfo::default()
                        .render_pass(self.down_pass)
                        .attachments(std::slice::from_ref(&view))
                        .width(self.extents[level as usize].width)
                        .height(self.extents[level as usize].height)
                        .layers(1),
                    None,
                )?);
            }

            device.reset_descriptor_pool(
                self.descriptor_pool,
                vk::DescriptorPoolResetFlags::empty(),
            )?;
            let sources: Vec<vk::ImageView> =
                std::iter::once(scene).chain(self.views.iter().copied()).collect();
            for source in sources {
                let set = device.allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(self.descriptor_pool)
                        .set_layouts(std::slice::from_ref(&self.descriptor_layout)),
                )?[0];
                let image_info = [vk::DescriptorImageInfo::default()
                    .image_view(source)
                    .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
                let sampler_info = [vk::DescriptorImageInfo::default().sampler(self.sampler)];
                device.update_descriptor_sets(
                    &[
                        vk::WriteDescriptorSet::default()
                            .dst_set(set)
                            .dst_binding(0)
                            .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                            .image_info(&image_info),
                        vk::WriteDescriptorSet::default()
                            .dst_set(set)
                            .dst_binding(1)
                            .descriptor_type(vk::DescriptorType::SAMPLER)
                            .image_info(&sampler_info),
                    ],
                    &[],
                );
                self.sets.push(set);
            }
            Ok(())
        }
    }

    /// Records the whole pyramid walk. Runs between the water pass and the
    /// swapchain pass; the scene HDR image must be SHADER_READ_ONLY.
    pub unsafe fn record(&self, device: &ash::Device, cmd: vk::CommandBuffer) {
        unsafe {
            let levels = self.extents.len();
            // Down: scene -> mip 0 -> ... -> mip N-1 (first pass thresholds).
            for level in 0..levels {
                let (src_extent, set) = if level == 0 {
                    // Source texel of the scene = 2x the mip-0 texel.
                    (
                        vk::Extent2D {
                            width: self.extents[0].width * 2,
                            height: self.extents[0].height * 2,
                        },
                        self.sets[0],
                    )
                } else {
                    (self.extents[level - 1], self.sets[level])
                };
                self.blit(
                    device,
                    cmd,
                    self.down_pass,
                    self.down_pipeline,
                    level,
                    set,
                    src_extent,
                    level == 0,
                );
            }
            // Up: blend each mip additively into the one above it.
            for level in (0..levels.saturating_sub(1)).rev() {
                self.blit(
                    device,
                    cmd,
                    self.up_pass,
                    self.up_pipeline,
                    level,
                    self.sets[level + 2],
                    self.extents[level + 1],
                    false,
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn blit(
        &self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        pass: vk::RenderPass,
        pipeline: vk::Pipeline,
        target: usize,
        set: vk::DescriptorSet,
        src_extent: vk::Extent2D,
        first: bool,
    ) {
        unsafe {
            let extent = self.extents[target];
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
                    .render_pass(pass)
                    .framebuffer(self.framebuffers[target])
                    .render_area(extent.into()),
                vk::SubpassContents::INLINE,
            );
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline);
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout,
                0,
                &[set],
                &[],
            );
            let push = Vec4::new(
                1.0 / src_extent.width as f32,
                1.0 / src_extent.height as f32,
                first as u32 as f32,
                0.0,
            );
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

    unsafe fn destroy_images(&mut self, device: &ash::Device, allocator: &mut Allocator) {
        unsafe {
            for framebuffer in self.framebuffers.drain(..) {
                device.destroy_framebuffer(framebuffer, None);
            }
            for view in self.views.drain(..) {
                device.destroy_image_view(view, None);
            }
            if self.image != vk::Image::null() {
                device.destroy_image(self.image, None);
                self.image = vk::Image::null();
            }
            if let Some(allocation) = self.allocation.take() {
                let _ = allocator.free(allocation);
            }
            self.sets.clear();
            self.extents.clear();
        }
    }

    pub unsafe fn destroy(&mut self, device: &ash::Device, allocator: &mut Allocator) {
        unsafe {
            self.destroy_images(device, allocator);
            device.destroy_pipeline(self.down_pipeline, None);
            device.destroy_pipeline(self.up_pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            device.destroy_sampler(self.sampler, None);
            device.destroy_descriptor_pool(self.descriptor_pool, None);
            device.destroy_descriptor_set_layout(self.descriptor_layout, None);
            device.destroy_render_pass(self.up_pass, None);
            device.destroy_render_pass(self.down_pass, None);
        }
    }
}

/// One single-attachment pass over an HDR mip. Downsamples overwrite the
/// mip; upsamples load it and the pipeline blends additively. Entry waits
/// for prior fragment reads/writes of the pyramid (and, cross-frame, the
/// tonemap's read of mip 0); exit makes the write visible to samplers.
unsafe fn create_pass(device: &ash::Device, load: bool) -> Result<vk::RenderPass> {
    unsafe {
        let attachment = vk::AttachmentDescription::default()
            .format(HDR_FORMAT)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(if load { vk::AttachmentLoadOp::LOAD } else { vk::AttachmentLoadOp::DONT_CARE })
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(if load {
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
            } else {
                vk::ImageLayout::UNDEFINED
            })
            .final_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
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
                        | vk::PipelineStageFlags::FRAGMENT_SHADER,
                )
                .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
                .dst_stage_mask(
                    vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                        | vk::PipelineStageFlags::FRAGMENT_SHADER,
                )
                .dst_access_mask(
                    vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                        | vk::AccessFlags::COLOR_ATTACHMENT_READ
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
        Ok(device.create_render_pass(
            &vk::RenderPassCreateInfo::default()
                .attachments(std::slice::from_ref(&attachment))
                .subpasses(std::slice::from_ref(&subpass))
                .dependencies(&dependencies),
            None,
        )?)
    }
}

unsafe fn create_pipeline(
    device: &ash::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
    fragment_entry: &std::ffi::CStr,
    additive: bool,
) -> Result<vk::Pipeline> {
    unsafe {
        let code = ash::util::read_spv(&mut std::io::Cursor::new(BLOOM_SPV))?;
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
                .name(fragment_entry),
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
            .color_write_mask(vk::ColorComponentFlags::RGBA)
            .blend_enable(additive)
            .src_color_blend_factor(vk::BlendFactor::ONE)
            .dst_color_blend_factor(vk::BlendFactor::ONE)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ZERO)
            .alpha_blend_op(vk::BlendOp::ADD);
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
