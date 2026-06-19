//! Screen-space HUD text rendering (debug overlay, §11).

use anyhow::{Context as _, Result};
use ash::vk;
use gpu_allocator::MemoryLocation;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme, Allocator};

use crate::chunk_renderer::{as_bytes, create_filled_buffer};
use crate::context::VulkanContext;
use crate::font;

const UI_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ui.spv"));

/// Maximum UI primitives (glyphs + quads + polys) per frame; excess is
/// truncated. Item icons add several polys per inventory slot, so this is
/// generous.
const MAX_GLYPHS: usize = 4096;
/// Drop-shadow offset in pixels.
const SHADOW_OFFSET: f32 = 2.0;

#[repr(C)]
#[derive(Clone, Copy)]
struct UiVertex {
    pos: [f32; 2],
    uv: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct UiPush {
    screen: [f32; 2],
    offset: [f32; 2],
    color: [f32; 4],
}

/// A positioned text run in framebuffer pixels (top-left origin).
#[derive(Debug, Clone)]
pub struct UiText {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub scale: f32,
}

/// A solid-colored rectangle in framebuffer pixels (top-left origin).
#[derive(Debug, Clone, Copy)]
pub struct UiQuad {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub color: [f32; 4],
}

/// A solid-colored quad with four arbitrary corners (framebuffer pixels), for
/// the angled faces of isometric item icons. Corner order is top-left,
/// top-right, bottom-left, bottom-right; it triangulates as (TL,TR,BL) +
/// (BL,TR,BR), so any convex quad given in that order fills correctly.
#[derive(Debug, Clone, Copy)]
pub struct UiPoly {
    pub corners: [[f32; 2]; 4],
    pub color: [f32; 4],
}

pub struct UiRenderer {
    descriptor_set_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    descriptor_set: vk::DescriptorSet,
    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    atlas_image: vk::Image,
    atlas_allocation: Option<Allocation>,
    atlas_view: vk::ImageView,
    sampler: vk::Sampler,
    /// One host-visible vertex buffer per frame in flight.
    vertex_buffers: Vec<(vk::Buffer, Allocation)>,
}

impl UiRenderer {
    pub unsafe fn new(
        ctx: &VulkanContext,
        allocator: &mut Allocator,
        render_pass: vk::RenderPass,
        command_pool: vk::CommandPool,
        frames_in_flight: usize,
    ) -> Result<Self> {
        unsafe {
            let device = &ctx.device;

            let (atlas_image, atlas_allocation, atlas_view) =
                upload_font_atlas(ctx, allocator, command_pool)?;
            let sampler = device.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::NEAREST)
                    .min_filter(vk::Filter::NEAREST)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
                None,
            )?;

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
            let descriptor_set_layout = device.create_descriptor_set_layout(
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
                    .set_layouts(std::slice::from_ref(&descriptor_set_layout)),
            )?[0];
            let image_info = vk::DescriptorImageInfo::default()
                .image_view(atlas_view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
            let sampler_info = vk::DescriptorImageInfo::default().sampler(sampler);
            device.update_descriptor_sets(
                &[
                    vk::WriteDescriptorSet::default()
                        .dst_set(descriptor_set)
                        .dst_binding(0)
                        .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                        .image_info(std::slice::from_ref(&image_info)),
                    vk::WriteDescriptorSet::default()
                        .dst_set(descriptor_set)
                        .dst_binding(1)
                        .descriptor_type(vk::DescriptorType::SAMPLER)
                        .image_info(std::slice::from_ref(&sampler_info)),
                ],
                &[],
            );

            let push_range = vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
                .size(size_of::<UiPush>() as u32);
            let pipeline_layout = device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(std::slice::from_ref(&descriptor_set_layout))
                    .push_constant_ranges(std::slice::from_ref(&push_range)),
                None,
            )?;
            let pipeline = create_pipeline(device, render_pass, pipeline_layout)?;

            let mut vertex_buffers = Vec::with_capacity(frames_in_flight);
            for i in 0..frames_in_flight {
                let size = (MAX_GLYPHS * 6 * size_of::<UiVertex>()) as u64;
                let buffer = device.create_buffer(
                    &vk::BufferCreateInfo::default()
                        .size(size)
                        .usage(vk::BufferUsageFlags::VERTEX_BUFFER)
                        .sharing_mode(vk::SharingMode::EXCLUSIVE),
                    None,
                )?;
                let requirements = device.get_buffer_memory_requirements(buffer);
                let allocation = allocator.allocate(&AllocationCreateDesc {
                    name: &format!("hud vertices {i}"),
                    requirements,
                    location: MemoryLocation::CpuToGpu,
                    linear: true,
                    allocation_scheme: AllocationScheme::GpuAllocatorManaged,
                })?;
                device.bind_buffer_memory(buffer, allocation.memory(), allocation.offset())?;
                vertex_buffers.push((buffer, allocation));
            }

            Ok(Self {
                descriptor_set_layout,
                descriptor_pool,
                descriptor_set,
                pipeline_layout,
                pipeline,
                atlas_image,
                atlas_allocation: Some(atlas_allocation),
                atlas_view,
                sampler,
                vertex_buffers,
            })
        }
    }

    /// Writes the text runs and colored quads into this frame's buffer and
    /// records the draws. Must run inside the render pass with viewport set.
    pub unsafe fn record(
        &mut self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        slot: usize,
        extent: vk::Extent2D,
        texts: &[UiText],
        quads: &[UiQuad],
        polys: &[UiPoly],
    ) {
        unsafe {
            let mut glyphs = Vec::new();
            for run in texts {
                glyphs.extend(font::layout(&run.text, run.x, run.y, run.scale));
            }
            let glyph_count = glyphs.len().min(MAX_GLYPHS);
            let quad_count = quads.len().min(MAX_GLYPHS - glyph_count);
            let poly_count = polys.len().min(MAX_GLYPHS - glyph_count - quad_count);
            if glyph_count + quad_count + poly_count == 0 {
                return;
            }

            let mut vertices =
                Vec::with_capacity((glyph_count + quad_count + poly_count) * 6);
            let mut emit = |x: f32, y: f32, w: f32, h: f32, uv: (f32, f32, f32, f32)| {
                let (u0, v0, u1, v1) = uv;
                let a = UiVertex { pos: [x, y], uv: [u0, v0] };
                let b = UiVertex { pos: [x + w, y], uv: [u1, v0] };
                let c = UiVertex { pos: [x, y + h], uv: [u0, v1] };
                let d = UiVertex { pos: [x + w, y + h], uv: [u1, v1] };
                vertices.extend_from_slice(&[a, b, c, c, b, d]);
            };
            for g in &glyphs[..glyph_count] {
                emit(g.x, g.y, g.w, g.h, (g.u0, g.v0, g.u1, g.v1));
            }
            let solid = font::solid_uv();
            for q in &quads[..quad_count] {
                emit(q.x, q.y, q.w, q.h, solid);
            }
            // Arbitrary-corner filled quads (icon faces). `emit`'s borrow of
            // `vertices` has ended, so push directly. Corner order TL,TR,BL,BR.
            let (su0, sv0, su1, sv1) = solid;
            for p in &polys[..poly_count] {
                let c = p.corners;
                let a = UiVertex { pos: c[0], uv: [su0, sv0] };
                let b = UiVertex { pos: c[1], uv: [su1, sv0] };
                let cc = UiVertex { pos: c[2], uv: [su0, sv1] };
                let d = UiVertex { pos: c[3], uv: [su1, sv1] };
                vertices.extend_from_slice(&[a, b, cc, cc, b, d]);
            }

            let (buffer, allocation) = &mut self.vertex_buffers[slot];
            let bytes = as_bytes(&vertices);
            allocation
                .mapped_slice_mut()
                .expect("hud buffer is host visible")[..bytes.len()]
                .copy_from_slice(bytes);

            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout,
                0,
                &[self.descriptor_set],
                &[],
            );
            device.cmd_bind_vertex_buffers(cmd, 0, &[*buffer], &[0]);

            let screen = [extent.width as f32, extent.height as f32];
            let mut draw = |push: UiPush, first: u32, count: u32| {
                device.cmd_push_constants(
                    cmd,
                    self.pipeline_layout,
                    vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                    0,
                    as_bytes(std::slice::from_ref(&push)),
                );
                device.cmd_draw(cmd, count, 1, first, 0);
            };

            // Colored quads first (panels), one draw each for its color.
            for (i, q) in quads[..quad_count].iter().enumerate() {
                draw(
                    UiPush { screen, offset: [0.0, 0.0], color: q.color },
                    ((glyph_count + i) * 6) as u32,
                    6,
                );
            }
            // Icon faces on top of the panel quads, still under the text.
            for (i, p) in polys[..poly_count].iter().enumerate() {
                draw(
                    UiPush { screen, offset: [0.0, 0.0], color: p.color },
                    ((glyph_count + quad_count + i) * 6) as u32,
                    6,
                );
            }
            // Text on top: drop shadow, then white.
            if glyph_count > 0 {
                for (offset, color) in [
                    ([SHADOW_OFFSET, SHADOW_OFFSET], [0.0, 0.0, 0.0, 0.85]),
                    ([0.0, 0.0], [1.0, 1.0, 1.0, 1.0]),
                ] {
                    draw(UiPush { screen, offset, color }, 0, (glyph_count * 6) as u32);
                }
            }
        }
    }

    pub unsafe fn destroy(&mut self, device: &ash::Device, allocator: &mut Allocator) {
        unsafe {
            for (buffer, allocation) in self.vertex_buffers.drain(..) {
                let _ = allocator.free(allocation);
                device.destroy_buffer(buffer, None);
            }
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            device.destroy_descriptor_pool(self.descriptor_pool, None);
            device.destroy_descriptor_set_layout(self.descriptor_set_layout, None);
            device.destroy_sampler(self.sampler, None);
            device.destroy_image_view(self.atlas_view, None);
            if let Some(allocation) = self.atlas_allocation.take() {
                let _ = allocator.free(allocation);
            }
            device.destroy_image(self.atlas_image, None);
        }
    }
}

unsafe fn upload_font_atlas(
    ctx: &VulkanContext,
    allocator: &mut Allocator,
    command_pool: vk::CommandPool,
) -> Result<(vk::Image, Allocation, vk::ImageView)> {
    unsafe {
        let device = &ctx.device;
        let pixels = font::build_atlas();
        let extent = vk::Extent3D {
            width: font::atlas_width(),
            height: font::atlas_height(),
            depth: 1,
        };

        let image = device.create_image(
            &vk::ImageCreateInfo::default()
                .image_type(vk::ImageType::TYPE_2D)
                .format(vk::Format::R8_UNORM)
                .extent(extent)
                .mip_levels(1)
                .array_layers(1)
                .samples(vk::SampleCountFlags::TYPE_1)
                .tiling(vk::ImageTiling::OPTIMAL)
                .usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED)
                .initial_layout(vk::ImageLayout::UNDEFINED),
            None,
        )?;
        let requirements = device.get_image_memory_requirements(image);
        let allocation = allocator.allocate(&AllocationCreateDesc {
            name: "font atlas",
            requirements,
            location: MemoryLocation::GpuOnly,
            linear: false,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })?;
        device.bind_image_memory(image, allocation.memory(), allocation.offset())?;

        let mut staging = create_filled_buffer(
            ctx,
            allocator,
            vk::BufferUsageFlags::TRANSFER_SRC,
            &pixels,
            "font staging",
        )?;

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

        let range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1)
            .layer_count(1);
        let to_transfer = vk::ImageMemoryBarrier::default()
            .image(image)
            .subresource_range(range)
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .src_access_mask(vk::AccessFlags::empty())
            .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE);
        device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::TRANSFER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[to_transfer],
        );
        let copy = vk::BufferImageCopy::default()
            .image_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .layer_count(1),
            )
            .image_extent(extent);
        device.cmd_copy_buffer_to_image(
            cmd,
            staging.buffer,
            image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &[copy],
        );
        let to_sampled = vk::ImageMemoryBarrier::default()
            .image(image)
            .subresource_range(range)
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(vk::AccessFlags::SHADER_READ);
        device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::FRAGMENT_SHADER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[to_sampled],
        );
        device.end_command_buffer(cmd)?;
        let submit = vk::SubmitInfo::default().command_buffers(std::slice::from_ref(&cmd));
        device.queue_submit(ctx.queue, &[submit], vk::Fence::null())?;
        device.queue_wait_idle(ctx.queue)?;
        device.free_command_buffers(command_pool, &[cmd]);
        staging.destroy(device, allocator);

        let view = device.create_image_view(
            &vk::ImageViewCreateInfo::default()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(vk::Format::R8_UNORM)
                .subresource_range(range),
            None,
        )?;
        Ok((image, allocation, view))
    }
}

unsafe fn create_pipeline(
    device: &ash::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> Result<vk::Pipeline> {
    unsafe {
        let code = ash::util::read_spv(&mut std::io::Cursor::new(UI_SPV))
            .context("reading ui shader")?;
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

        let binding = vk::VertexInputBindingDescription::default()
            .stride(size_of::<UiVertex>() as u32)
            .input_rate(vk::VertexInputRate::VERTEX);
        let attributes = [
            vk::VertexInputAttributeDescription::default()
                .location(0)
                .format(vk::Format::R32G32_SFLOAT),
            vk::VertexInputAttributeDescription::default()
                .location(1)
                .format(vk::Format::R32G32_SFLOAT)
                .offset(8),
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(std::slice::from_ref(&binding))
            .vertex_attribute_descriptions(&attributes);

        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport_state = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        // HUD draws over everything: no depth test or write.
        let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default();
        let blend_attachment = vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .alpha_blend_op(vk::BlendOp::ADD)
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
