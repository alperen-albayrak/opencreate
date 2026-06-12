//! Auto-exposure (graphics roadmap stage E): the camera's eye adapts.
//!
//! Each frame renders a 16x16 log2-luminance grid of the HDR scene into an
//! R32_SFLOAT image and copies it to a host-visible buffer. When the
//! frame's fence comes around again (FRAMES_IN_FLIGHT later), the CPU
//! averages the grid — a geometric mean, robust against the sun — and
//! eases the exposure toward `KEY / average`. The tonemap multiplies the
//! scene by the result: caves brighten as your eyes adjust, stepping back
//! into daylight dazzles briefly.

use anyhow::Result;
use ash::vk;
use gpu_allocator::MemoryLocation;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme, Allocator};

use crate::context::VulkanContext;

const EXPOSURE_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/exposure.spv"));

/// Measurement grid edge; 256 samples across the screen.
const SIZE: u32 = 16;
/// Average scene luminance the exposure steers toward.
const KEY: f32 = 0.32;
/// Exposure bounds: never crush daylight, never turn night into day.
/// The floor is gentle — stopping down harder than this crushes deep
/// ocean and shaded terrain to black whenever bright sky fills the view.
const MIN_EXPOSURE: f32 = 0.75;
const MAX_EXPOSURE: f32 = 2.4;
/// Adaptation rate per second (eased exponentially).
const SPEED: f32 = 1.8;

pub struct ExposurePass {
    render_pass: vk::RenderPass,
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    descriptor_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    descriptor_set: vk::DescriptorSet,
    sampler: vk::Sampler,
    image: vk::Image,
    image_allocation: Option<Allocation>,
    view: vk::ImageView,
    framebuffer: vk::Framebuffer,
    /// One host-visible readback buffer per frame in flight.
    readbacks: Vec<(vk::Buffer, Allocation)>,
    exposure: f32,
    last_time: Option<f32>,
}

impl ExposurePass {
    pub unsafe fn new(
        ctx: &VulkanContext,
        allocator: &mut Allocator,
        scene: vk::ImageView,
        frames_in_flight: usize,
    ) -> Result<Self> {
        unsafe {
            let device = &ctx.device;
            let render_pass = create_pass(device)?;

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
            let pool_sizes = [
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(1),
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
            let sampler = device.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::LINEAR)
                    .min_filter(vk::Filter::LINEAR)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
                None,
            )?;

            let pipeline_layout = device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(std::slice::from_ref(&descriptor_layout)),
                None,
            )?;
            let pipeline = create_pipeline(device, render_pass, pipeline_layout)?;

            // The tiny measurement target.
            let image = device.create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(vk::Format::R32_SFLOAT)
                    .extent(vk::Extent3D { width: SIZE, height: SIZE, depth: 1 })
                    .mip_levels(1)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(
                        vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
                    )
                    .initial_layout(vk::ImageLayout::UNDEFINED),
                None,
            )?;
            let requirements = device.get_image_memory_requirements(image);
            let image_allocation = allocator.allocate(&AllocationCreateDesc {
                name: "exposure grid",
                requirements,
                location: MemoryLocation::GpuOnly,
                linear: false,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })?;
            device.bind_image_memory(image, image_allocation.memory(), image_allocation.offset())?;
            let view = device.create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(vk::Format::R32_SFLOAT)
                    .subresource_range(
                        vk::ImageSubresourceRange::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .level_count(1)
                            .layer_count(1),
                    ),
                None,
            )?;
            let framebuffer = device.create_framebuffer(
                &vk::FramebufferCreateInfo::default()
                    .render_pass(render_pass)
                    .attachments(std::slice::from_ref(&view))
                    .width(SIZE)
                    .height(SIZE)
                    .layers(1),
                None,
            )?;

            let bytes = (SIZE * SIZE * 4) as u64;
            let mut readbacks = Vec::new();
            for i in 0..frames_in_flight {
                let buffer = device.create_buffer(
                    &vk::BufferCreateInfo::default()
                        .size(bytes)
                        .usage(vk::BufferUsageFlags::TRANSFER_DST),
                    None,
                )?;
                let requirements = device.get_buffer_memory_requirements(buffer);
                let allocation = allocator.allocate(&AllocationCreateDesc {
                    name: &format!("exposure readback {i}"),
                    requirements,
                    location: MemoryLocation::GpuToCpu,
                    linear: true,
                    allocation_scheme: AllocationScheme::GpuAllocatorManaged,
                })?;
                device.bind_buffer_memory(buffer, allocation.memory(), allocation.offset())?;
                readbacks.push((buffer, allocation));
            }

            let pass = Self {
                render_pass,
                pipeline,
                pipeline_layout,
                descriptor_layout,
                descriptor_pool,
                descriptor_set,
                sampler,
                image,
                image_allocation: Some(image_allocation),
                view,
                framebuffer,
                readbacks,
                exposure: 1.0,
                last_time: None,
            };
            pass.bind_input(device, scene);
            Ok(pass)
        }
    }

    /// Points the measurement at the (re)created HDR image. Call while the
    /// device is idle.
    pub unsafe fn bind_input(&self, device: &ash::Device, scene: vk::ImageView) {
        unsafe {
            let image_info = [vk::DescriptorImageInfo::default()
                .image_view(scene)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            let sampler_info = [vk::DescriptorImageInfo::default().sampler(self.sampler)];
            device.update_descriptor_sets(
                &[
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
                ],
                &[],
            );
        }
    }

    /// Reads the grid this slot's previous submission produced (the fence
    /// was just waited on) and eases the exposure toward its average.
    /// `time` is the frame clock in seconds.
    pub fn adapt(&mut self, slot: usize, time: f32) -> f32 {
        let (_, allocation) = &self.readbacks[slot];
        if let Some(mapped) = allocation.mapped_slice() {
            let mut sum = 0.0f32;
            let mut count = 0;
            for chunk in mapped.chunks_exact(4).take((SIZE * SIZE) as usize) {
                let v = f32::from_le_bytes(chunk.try_into().expect("4-byte chunks"));
                // Skip never-written zeros from the first frames... log2
                // values are negative for dim scenes, 0.0 exactly is rare
                // but valid; accept everything finite instead.
                if v.is_finite() {
                    sum += v;
                    count += 1;
                }
            }
            if count > 0 {
                let average = (sum / count as f32).exp2();
                let target = (KEY / average.max(0.0001)).clamp(MIN_EXPOSURE, MAX_EXPOSURE);
                let dt = self
                    .last_time
                    .map_or(0.016, |last| (time - last).clamp(0.0, 0.25));
                let ease = 1.0 - (-SPEED * dt).exp();
                self.exposure += (target - self.exposure) * ease;
            }
        }
        self.last_time = Some(time);
        self.exposure
    }

    /// Records the measurement: the tiny pass plus the copy into this
    /// slot's readback buffer. The scene HDR image must be SHADER_READ_ONLY
    /// (i.e. run after the water pass).
    pub unsafe fn record(&self, device: &ash::Device, cmd: vk::CommandBuffer, slot: usize) {
        unsafe {
            let extent = vk::Extent2D { width: SIZE, height: SIZE };
            device.cmd_set_viewport(
                cmd,
                0,
                &[vk::Viewport::default()
                    .width(SIZE as f32)
                    .height(SIZE as f32)
                    .max_depth(1.0)],
            );
            device.cmd_set_scissor(cmd, 0, &[extent.into()]);
            device.cmd_begin_render_pass(
                cmd,
                &vk::RenderPassBeginInfo::default()
                    .render_pass(self.render_pass)
                    .framebuffer(self.framebuffer)
                    .render_area(extent.into()),
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
            device.cmd_draw(cmd, 3, 1, 0, 0);
            device.cmd_end_render_pass(cmd);

            // The render pass left the image TRANSFER_SRC; the fence wait
            // makes the buffer's contents host-visible next time around.
            device.cmd_copy_image_to_buffer(
                cmd,
                self.image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                self.readbacks[slot].0,
                &[vk::BufferImageCopy::default()
                    .image_subresource(
                        vk::ImageSubresourceLayers::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .layer_count(1),
                    )
                    .image_extent(vk::Extent3D { width: SIZE, height: SIZE, depth: 1 })],
            );
        }
    }

    pub unsafe fn destroy(&mut self, device: &ash::Device, allocator: &mut Allocator) {
        unsafe {
            for (buffer, allocation) in self.readbacks.drain(..) {
                device.destroy_buffer(buffer, None);
                let _ = allocator.free(allocation);
            }
            device.destroy_framebuffer(self.framebuffer, None);
            device.destroy_image_view(self.view, None);
            device.destroy_image(self.image, None);
            if let Some(allocation) = self.image_allocation.take() {
                let _ = allocator.free(allocation);
            }
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            device.destroy_sampler(self.sampler, None);
            device.destroy_descriptor_pool(self.descriptor_pool, None);
            device.destroy_descriptor_set_layout(self.descriptor_layout, None);
            device.destroy_render_pass(self.render_pass, None);
        }
    }
}

/// Renders the grid and hands it to the transfer that follows. Entry waits
/// for the previous frame's copy (WAR on the image); exit makes the write
/// visible to TRANSFER.
unsafe fn create_pass(device: &ash::Device) -> Result<vk::RenderPass> {
    unsafe {
        let attachment = vk::AttachmentDescription::default()
            .format(vk::Format::R32_SFLOAT)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::DONT_CARE)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL);
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
                    vk::PipelineStageFlags::TRANSFER | vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                )
                .src_access_mask(vk::AccessFlags::empty())
                .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
                .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE),
            vk::SubpassDependency::default()
                .src_subpass(0)
                .dst_subpass(vk::SUBPASS_EXTERNAL)
                .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
                .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags::TRANSFER)
                .dst_access_mask(vk::AccessFlags::TRANSFER_READ),
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
) -> Result<vk::Pipeline> {
    unsafe {
        let code = ash::util::read_spv(&mut std::io::Cursor::new(EXPOSURE_SPV))?;
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
