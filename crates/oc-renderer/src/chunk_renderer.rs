//! Graphics pipeline and GPU resources for drawing chunk meshes.

use std::collections::HashMap;

use anyhow::{Context as _, Result};
use ash::vk;
use glam::{DVec3, Mat4};
use gpu_allocator::MemoryLocation;
use gpu_allocator::vulkan::{
    Allocation, AllocationCreateDesc, AllocationScheme, Allocator,
};
use oc_core::{SECTION_SIZE, SectionPos};

use crate::context::VulkanContext;
use crate::mesh::{ChunkMesh, PackedVertex};
use crate::texture;
use crate::FRAMES_IN_FLIGHT;

const CHUNK_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/chunk.spv"));

pub struct GpuBuffer {
    pub buffer: vk::Buffer,
    allocation: Option<Allocation>,
}

impl GpuBuffer {
    unsafe fn destroy(&mut self, device: &ash::Device, allocator: &mut Allocator) {
        unsafe {
            if let Some(allocation) = self.allocation.take() {
                let _ = allocator.free(allocation);
            }
            device.destroy_buffer(self.buffer, None);
        }
    }
}

struct ChunkMeshGpu {
    vertex: GpuBuffer,
    index: GpuBuffer,
    index_count: u32,
    origin: DVec3,
}

/// Owns the chunk pipeline, block texture array and uploaded chunk meshes.
pub struct ChunkRenderer {
    descriptor_set_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    descriptor_set: vk::DescriptorSet,
    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    texture_image: vk::Image,
    texture_allocation: Option<Allocation>,
    texture_view: vk::ImageView,
    sampler: vk::Sampler,
    chunks: HashMap<SectionPos, ChunkMeshGpu>,
    /// Buffers replaced or removed while the GPU may still read them, tagged
    /// with the frame counter at retirement. Freed once that frame's
    /// command buffers are provably finished.
    retired: Vec<(u64, GpuBuffer)>,
}

impl ChunkRenderer {
    pub unsafe fn new(
        ctx: &VulkanContext,
        allocator: &mut Allocator,
        render_pass: vk::RenderPass,
        command_pool: vk::CommandPool,
    ) -> Result<Self> {
        unsafe {
            let device = &ctx.device;

            // Block texture array + sampler.
            let (texture_image, texture_allocation, texture_view) =
                upload_block_textures(ctx, allocator, command_pool)?;
            let sampler = device.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::NEAREST)
                    .min_filter(vk::Filter::NEAREST)
                    .address_mode_u(vk::SamplerAddressMode::REPEAT)
                    .address_mode_v(vk::SamplerAddressMode::REPEAT)
                    .address_mode_w(vk::SamplerAddressMode::REPEAT),
                None,
            )?;

            // Descriptors: binding 0 = sampled image, binding 1 = sampler
            // (WGSL separates textures and samplers).
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
                .image_view(texture_view)
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

            // Pipeline.
            let push_range = vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::VERTEX)
                .size(size_of::<Mat4>() as u32);
            let pipeline_layout = device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(std::slice::from_ref(&descriptor_set_layout))
                    .push_constant_ranges(std::slice::from_ref(&push_range)),
                None,
            )?;
            let pipeline = create_pipeline(device, render_pass, pipeline_layout)?;

            Ok(Self {
                descriptor_set_layout,
                descriptor_pool,
                descriptor_set,
                pipeline_layout,
                pipeline,
                texture_image,
                texture_allocation: Some(texture_allocation),
                texture_view,
                sampler,
                chunks: HashMap::new(),
                retired: Vec::new(),
            })
        }
    }

    /// Uploads a section mesh, replacing any previous mesh at `pos`. An empty
    /// mesh just removes the old one.
    pub unsafe fn set_chunk(
        &mut self,
        ctx: &VulkanContext,
        allocator: &mut Allocator,
        pos: SectionPos,
        mesh: &ChunkMesh,
        frame: u64,
    ) -> Result<()> {
        unsafe {
            self.remove_chunk(pos, frame);
            if mesh.indices.is_empty() {
                return Ok(());
            }

            let vertex = create_filled_buffer(
                ctx,
                allocator,
                vk::BufferUsageFlags::VERTEX_BUFFER,
                as_bytes(&mesh.vertices),
                "chunk vertices",
            )?;
            let index = create_filled_buffer(
                ctx,
                allocator,
                vk::BufferUsageFlags::INDEX_BUFFER,
                as_bytes(&mesh.indices),
                "chunk indices",
            )?;
            self.chunks.insert(pos, ChunkMeshGpu {
                vertex,
                index,
                index_count: mesh.indices.len() as u32,
                origin: (pos * SECTION_SIZE).as_dvec3(),
            });
            Ok(())
        }
    }

    /// Drops the mesh at `pos`; its buffers are freed once the GPU is done.
    pub fn remove_chunk(&mut self, pos: SectionPos, frame: u64) {
        if let Some(old) = self.chunks.remove(&pos) {
            self.retired.push((frame, old.vertex));
            self.retired.push((frame, old.index));
        }
    }

    /// Frees retired buffers whose last possible use is at least
    /// `FRAMES_IN_FLIGHT` frames behind `frame` (i.e. provably complete).
    /// Call after waiting on the current frame's fence.
    pub unsafe fn collect_garbage(
        &mut self,
        device: &ash::Device,
        allocator: &mut Allocator,
        frame: u64,
    ) {
        unsafe {
            self.retired.retain_mut(|(retired_at, buffer)| {
                if *retired_at + FRAMES_IN_FLIGHT as u64 <= frame {
                    buffer.destroy(device, allocator);
                    false
                } else {
                    true
                }
            });
        }
    }

    /// Records draw commands. Must be called inside a render pass with
    /// dynamic viewport/scissor already set.
    pub unsafe fn record(
        &self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        view_proj: Mat4,
        camera_pos: DVec3,
    ) {
        unsafe {
            if self.chunks.is_empty() {
                return;
            }

            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout,
                0,
                &[self.descriptor_set],
                &[],
            );

            // TODO: frustum culling + pooled buffers with multi-draw-indirect
            // (§4); per-chunk binds are fine at current chunk counts.
            for chunk in self.chunks.values() {
                device.cmd_bind_vertex_buffers(cmd, 0, &[chunk.vertex.buffer], &[0]);
                device.cmd_bind_index_buffer(cmd, chunk.index.buffer, 0, vk::IndexType::UINT32);

                // Camera-relative rendering (§3): translation happens in f64
                // on the CPU; the GPU only ever sees camera-relative f32.
                let rel = (chunk.origin - camera_pos).as_vec3();
                let mvp = view_proj * Mat4::from_translation(rel);
                device.cmd_push_constants(
                    cmd,
                    self.pipeline_layout,
                    vk::ShaderStageFlags::VERTEX,
                    0,
                    as_bytes(std::slice::from_ref(&mvp)),
                );
                device.cmd_draw_indexed(cmd, chunk.index_count, 1, 0, 0, 0);
            }
        }
    }

    pub unsafe fn destroy(&mut self, device: &ash::Device, allocator: &mut Allocator) {
        unsafe {
            // Caller has already waited for device idle; everything is safe
            // to free immediately.
            for (_, mut chunk) in self.chunks.drain() {
                chunk.vertex.destroy(device, allocator);
                chunk.index.destroy(device, allocator);
            }
            for (_, mut buffer) in self.retired.drain(..) {
                buffer.destroy(device, allocator);
            }
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            device.destroy_descriptor_pool(self.descriptor_pool, None);
            device.destroy_descriptor_set_layout(self.descriptor_set_layout, None);
            device.destroy_sampler(self.sampler, None);
            device.destroy_image_view(self.texture_view, None);
            if let Some(allocation) = self.texture_allocation.take() {
                let _ = allocator.free(allocation);
            }
            device.destroy_image(self.texture_image, None);
        }
    }
}

fn as_bytes<T: Copy>(slice: &[T]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(slice.as_ptr().cast(), std::mem::size_of_val(slice)) }
}

unsafe fn create_filled_buffer(
    ctx: &VulkanContext,
    allocator: &mut Allocator,
    usage: vk::BufferUsageFlags,
    data: &[u8],
    name: &str,
) -> Result<GpuBuffer> {
    unsafe {
        let buffer = ctx.device.create_buffer(
            &vk::BufferCreateInfo::default()
                .size(data.len() as u64)
                .usage(usage)
                .sharing_mode(vk::SharingMode::EXCLUSIVE),
            None,
        )?;
        let requirements = ctx.device.get_buffer_memory_requirements(buffer);
        let mut allocation = allocator.allocate(&AllocationCreateDesc {
            name,
            requirements,
            location: MemoryLocation::CpuToGpu,
            linear: true,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })?;
        ctx.device
            .bind_buffer_memory(buffer, allocation.memory(), allocation.offset())?;
        allocation
            .mapped_slice_mut()
            .context("buffer not host-visible")?[..data.len()]
            .copy_from_slice(data);
        Ok(GpuBuffer {
            buffer,
            allocation: Some(allocation),
        })
    }
}

unsafe fn upload_block_textures(
    ctx: &VulkanContext,
    allocator: &mut Allocator,
    command_pool: vk::CommandPool,
) -> Result<(vk::Image, Allocation, vk::ImageView)> {
    unsafe {
        let device = &ctx.device;
        let pixels = texture::build_block_textures();
        let extent = vk::Extent3D {
            width: texture::TEXTURE_SIZE,
            height: texture::TEXTURE_SIZE,
            depth: 1,
        };

        let image = device.create_image(
            &vk::ImageCreateInfo::default()
                .image_type(vk::ImageType::TYPE_2D)
                .format(vk::Format::R8G8B8A8_SRGB)
                .extent(extent)
                .mip_levels(1)
                .array_layers(texture::LAYER_COUNT)
                .samples(vk::SampleCountFlags::TYPE_1)
                .tiling(vk::ImageTiling::OPTIMAL)
                .usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED)
                .initial_layout(vk::ImageLayout::UNDEFINED),
            None,
        )?;
        let requirements = device.get_image_memory_requirements(image);
        let allocation = allocator.allocate(&AllocationCreateDesc {
            name: "block textures",
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
            "texture staging",
        )?;

        // One-time upload: UNDEFINED -> TRANSFER_DST -> copy -> SHADER_READ_ONLY.
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

        let subresource_range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1)
            .layer_count(texture::LAYER_COUNT);
        let to_transfer = vk::ImageMemoryBarrier::default()
            .image(image)
            .subresource_range(subresource_range)
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
                    .layer_count(texture::LAYER_COUNT),
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
            .subresource_range(subresource_range)
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
                .view_type(vk::ImageViewType::TYPE_2D_ARRAY)
                .format(vk::Format::R8G8B8A8_SRGB)
                .subresource_range(subresource_range),
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
        let code = ash::util::read_spv(&mut std::io::Cursor::new(CHUNK_SPV))?;
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
            .stride(size_of::<PackedVertex>() as u32)
            .input_rate(vk::VertexInputRate::VERTEX);
        let attribute = vk::VertexInputAttributeDescription::default()
            .location(0)
            .format(vk::Format::R32G32_UINT);
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(std::slice::from_ref(&binding))
            .vertex_attribute_descriptions(std::slice::from_ref(&attribute));

        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport_state = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            // Outward mesh faces are counter-clockwise per Vulkan's
            // framebuffer-space orientation rule (verified empirically:
            // CLOCKWISE+BACK culls the world away).
            .cull_mode(vk::CullModeFlags::BACK)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(true)
            .depth_write_enable(true)
            .depth_compare_op(vk::CompareOp::LESS);
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
