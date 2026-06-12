//! Graphics pipeline and GPU resources for drawing chunk meshes.

use std::collections::HashMap;

use anyhow::{Context as _, Result};
use ash::vk;
use glam::{DVec3, Mat4, Vec3, Vec4};
use gpu_allocator::MemoryLocation;
use gpu_allocator::vulkan::{
    Allocation, AllocationCreateDesc, AllocationScheme, Allocator,
};
use oc_core::{SECTION_SIZE, SectionPos};

use crate::context::VulkanContext;
use crate::mesh::{ChunkMesh, PackedVertex, SectionMeshes};
use crate::texture;
use crate::FRAMES_IN_FLIGHT;

const CHUNK_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/chunk.spv"));
const WATER_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/water.spv"));

pub struct GpuBuffer {
    pub buffer: vk::Buffer,
    allocation: Option<Allocation>,
}

impl GpuBuffer {
    pub(crate) unsafe fn destroy(&mut self, device: &ash::Device, allocator: &mut Allocator) {
        unsafe {
            if let Some(allocation) = self.allocation.take() {
                let _ = allocator.free(allocation);
            }
            device.destroy_buffer(self.buffer, None);
        }
    }
}

/// Push constants for the chunk pipeline; must match `chunk.wgsl`.
#[repr(C)]
#[derive(Clone, Copy)]
/// Exactly 128 bytes — the guaranteed push-constant minimum.
struct ChunkPush {
    mvp: Mat4,
    /// xyz: direction toward the sun; w: ambient light level.
    sun: Vec4,
    /// xyz: chunk origin mod 256 (caustic phase); w: time in seconds.
    params: Vec4,
    /// rgb: fog (horizon) color; w: fog saturation distance, blocks.
    fog: Vec4,
    /// xyz: chunk origin camera-relative (shadow lookups); w: unused.
    rel: Vec4,
}

/// Push constants for the water pipeline; must match `water.wgsl`.
/// Exactly 128 bytes — the guaranteed push-constant minimum.
#[repr(C)]
#[derive(Clone, Copy)]
struct WaterPush {
    mvp: Mat4,
    sun: Vec4,
    sky: Vec4,
    /// xyz: chunk origin camera-relative; w: time (seconds).
    rel: Vec4,
    /// xyz: chunk origin mod 256 (wave-phase anchor).
    wave_origin: Vec4,
}

/// Per-frame water uniforms (set 1 binding 3): what the SSR march needs.
#[repr(C)]
#[derive(Clone, Copy)]
struct WaterFrameData {
    /// Camera-relative world -> clip (the same projection chunks use).
    view_proj: Mat4,
    /// x, y: render extent in pixels; z: SSR enabled (1.0) or off; w: unused.
    params: Vec4,
}

struct DrawBuf {
    vertex: GpuBuffer,
    index: GpuBuffer,
    index_count: u32,
}

struct ChunkMeshGpu {
    solid: Option<DrawBuf>,
    water: Option<DrawBuf>,
    origin: DVec3,
}

/// Owns the chunk pipeline, block texture array and uploaded chunk meshes.
pub struct ChunkRenderer {
    descriptor_set_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    descriptor_set: vk::DescriptorSet,
    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    water_pipeline_layout: vk::PipelineLayout,
    water_pipeline: vk::Pipeline,
    water_depth_layout: vk::DescriptorSetLayout,
    water_depth_pool: vk::DescriptorPool,
    water_sets: Vec<vk::DescriptorSet>,
    water_uniforms: Vec<(vk::Buffer, Allocation)>,
    scene_sampler: vk::Sampler,
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
        water_pass: vk::RenderPass,
        command_pool: vk::CommandPool,
        shadow_layout: vk::DescriptorSetLayout,
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
                .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
                .size(size_of::<ChunkPush>() as u32);
            let chunk_sets = [descriptor_set_layout, shadow_layout];
            let pipeline_layout = device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&chunk_sets)
                    .push_constant_ranges(std::slice::from_ref(&push_range)),
                None,
            )?;
            let pipeline = create_pipeline(device, render_pass, pipeline_layout)?;

            // Water set 1: opaque depth + scene-color snapshot (rebound on
            // resize), a sampler, and a per-slot UBO for the SSR march.
            let water_bindings = [
                vk::DescriptorSetLayoutBinding::default()
                    .binding(0)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT),
                vk::DescriptorSetLayoutBinding::default()
                    .binding(1)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT),
                vk::DescriptorSetLayoutBinding::default()
                    .binding(2)
                    .descriptor_type(vk::DescriptorType::SAMPLER)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT),
                vk::DescriptorSetLayoutBinding::default()
                    .binding(3)
                    .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            ];
            let water_depth_layout = device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&water_bindings),
                None,
            )?;
            let slots = FRAMES_IN_FLIGHT as u32;
            let water_pool_sizes = [
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(2 * slots),
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::SAMPLER)
                    .descriptor_count(slots),
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::UNIFORM_BUFFER)
                    .descriptor_count(slots),
            ];
            let water_depth_pool = device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(slots)
                    .pool_sizes(&water_pool_sizes),
                None,
            )?;
            // Linear clamp sampler for reflection lookups.
            let scene_sampler = device.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::LINEAR)
                    .min_filter(vk::Filter::LINEAR)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
                None,
            )?;
            let mut water_sets = Vec::new();
            let mut water_uniforms = Vec::new();
            for i in 0..FRAMES_IN_FLIGHT {
                let set = device.allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(water_depth_pool)
                        .set_layouts(std::slice::from_ref(&water_depth_layout)),
                )?[0];
                let buffer = device.create_buffer(
                    &vk::BufferCreateInfo::default()
                        .size(size_of::<WaterFrameData>() as u64)
                        .usage(vk::BufferUsageFlags::UNIFORM_BUFFER),
                    None,
                )?;
                let requirements = device.get_buffer_memory_requirements(buffer);
                let alloc = allocator.allocate(&AllocationCreateDesc {
                    name: &format!("water ubo {i}"),
                    requirements,
                    location: MemoryLocation::CpuToGpu,
                    linear: true,
                    allocation_scheme: AllocationScheme::GpuAllocatorManaged,
                })?;
                device.bind_buffer_memory(buffer, alloc.memory(), alloc.offset())?;
                let buffer_info = [vk::DescriptorBufferInfo::default()
                    .buffer(buffer)
                    .range(size_of::<WaterFrameData>() as u64)];
                let sampler_info =
                    [vk::DescriptorImageInfo::default().sampler(scene_sampler)];
                device.update_descriptor_sets(
                    &[
                        vk::WriteDescriptorSet::default()
                            .dst_set(set)
                            .dst_binding(2)
                            .descriptor_type(vk::DescriptorType::SAMPLER)
                            .image_info(&sampler_info),
                        vk::WriteDescriptorSet::default()
                            .dst_set(set)
                            .dst_binding(3)
                            .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                            .buffer_info(&buffer_info),
                    ],
                    &[],
                );
                water_sets.push(set);
                water_uniforms.push((buffer, alloc));
            }

            let water_push = vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
                .size(size_of::<WaterPush>() as u32);
            let water_set_layouts = [descriptor_set_layout, water_depth_layout];
            let water_pipeline_layout = device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&water_set_layouts)
                    .push_constant_ranges(std::slice::from_ref(&water_push)),
                None,
            )?;
            let water_pipeline =
                create_water_pipeline(device, water_pass, water_pipeline_layout)?;

            Ok(Self {
                descriptor_set_layout,
                descriptor_pool,
                descriptor_set,
                pipeline_layout,
                pipeline,
                water_pipeline_layout,
                water_pipeline,
                water_depth_layout,
                water_depth_pool,
                water_sets,
                water_uniforms,
                scene_sampler,
                texture_image,
                texture_allocation: Some(texture_allocation),
                texture_view,
                sampler,
                chunks: HashMap::new(),
                retired: Vec::new(),
            })
        }
    }

    /// Uploads one part of a section mesh (or None when it has no faces).
    unsafe fn upload_part(
        ctx: &VulkanContext,
        allocator: &mut Allocator,
        mesh: &ChunkMesh,
        name: &str,
    ) -> Result<Option<DrawBuf>> {
        unsafe {
            if mesh.indices.is_empty() {
                return Ok(None);
            }
            let vertex = create_filled_buffer(
                ctx,
                allocator,
                vk::BufferUsageFlags::VERTEX_BUFFER,
                as_bytes(&mesh.vertices),
                name,
            )?;
            let index = create_filled_buffer(
                ctx,
                allocator,
                vk::BufferUsageFlags::INDEX_BUFFER,
                as_bytes(&mesh.indices),
                name,
            )?;
            Ok(Some(DrawBuf { vertex, index, index_count: mesh.indices.len() as u32 }))
        }
    }

    /// Uploads a section's meshes, replacing any previous ones at `pos`.
    /// Empty meshes just remove the old ones.
    pub unsafe fn set_chunk(
        &mut self,
        ctx: &VulkanContext,
        allocator: &mut Allocator,
        pos: SectionPos,
        meshes: &SectionMeshes,
        frame: u64,
    ) -> Result<()> {
        unsafe {
            self.remove_chunk(pos, frame);
            if meshes.is_empty() {
                return Ok(());
            }
            let solid = Self::upload_part(ctx, allocator, &meshes.solid, "chunk solid")?;
            let water = Self::upload_part(ctx, allocator, &meshes.water, "chunk water")?;
            self.chunks.insert(pos, ChunkMeshGpu {
                solid,
                water,
                origin: (pos * SECTION_SIZE).as_dvec3(),
            });
            Ok(())
        }
    }

    fn retire(&mut self, frame: u64, part: Option<DrawBuf>) {
        if let Some(buf) = part {
            self.retired.push((frame, buf.vertex));
            self.retired.push((frame, buf.index));
        }
    }

    /// Drops the mesh at `pos`; its buffers are freed once the GPU is done.
    pub fn remove_chunk(&mut self, pos: SectionPos, frame: u64) {
        if let Some(old) = self.chunks.remove(&pos) {
            self.retire(frame, old.solid);
            self.retire(frame, old.water);
        }
    }

    /// Drops every chunk mesh (leaving a world); buffers retire as usual.
    pub fn clear_chunks(&mut self, frame: u64) {
        let drained: Vec<_> = self.chunks.drain().map(|(_, old)| old).collect();
        for old in drained {
            self.retire(frame, old.solid);
            self.retire(frame, old.water);
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

    /// Records draw commands and returns how many chunks were drawn after
    /// culling. Must be called inside a render pass with dynamic
    /// viewport/scissor already set.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn record(
        &self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        view_proj: Mat4,
        camera_pos: DVec3,
        sun: Vec4,
        time: f32,
        fog: Vec4,
        shadow_set: vk::DescriptorSet,
    ) -> u32 {
        unsafe {
            if self.chunks.is_empty() {
                return 0;
            }
            let mut drawn = 0;

            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout,
                0,
                &[self.descriptor_set, shadow_set],
                &[],
            );

            let frustum = frustum_planes(view_proj);

            // TODO: pooled buffers with multi-draw-indirect (§4); per-chunk
            // binds are fine at current chunk counts.
            for chunk in self.chunks.values() {
                let Some(solid) = &chunk.solid else { continue };
                // CPU frustum culling at section granularity (§4), in
                // camera-relative space.
                let rel = (chunk.origin - camera_pos).as_vec3();
                if !aabb_intersects_frustum(&frustum, rel, rel + Vec3::splat(SECTION_SIZE as f32))
                {
                    continue;
                }
                device.cmd_bind_vertex_buffers(cmd, 0, &[solid.vertex.buffer], &[0]);
                device.cmd_bind_index_buffer(cmd, solid.index.buffer, 0, vk::IndexType::UINT32);

                // Camera-relative rendering (§3): translation happens in f64
                // on the CPU; the GPU only ever sees camera-relative f32.
                let origin = chunk.origin;
                let phase = Vec3::new(
                    origin.x.rem_euclid(256.0) as f32,
                    origin.y.rem_euclid(256.0) as f32,
                    origin.z.rem_euclid(256.0) as f32,
                );
                let push = ChunkPush {
                    mvp: view_proj * Mat4::from_translation(rel),
                    sun,
                    params: phase.extend(time),
                    fog,
                    rel: rel.extend(0.0),
                };
                device.cmd_push_constants(
                    cmd,
                    self.pipeline_layout,
                    vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                    0,
                    as_bytes(std::slice::from_ref(&push)),
                );
                device.cmd_draw_indexed(cmd, solid.index_count, 1, 0, 0, 0);
                drawn += 1;
            }
            drawn
        }
    }

    /// Records depth-only chunk draws for one shadow cascade. The caller
    /// has begun the cascade pass and bound the shadow pipeline; chunks
    /// are culled against the cascade's ortho box.
    pub unsafe fn record_shadow(
        &self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        layout: vk::PipelineLayout,
        cascade: Mat4,
        camera_pos: DVec3,
    ) {
        unsafe {
            // A section's bounding sphere in cascade NDC, per axis.
            let section_radius = (SECTION_SIZE as f32) * 0.5 * 1.7321;
            let pad_x = section_radius * cascade.x_axis.x.abs().max(cascade.y_axis.x.abs()).max(cascade.z_axis.x.abs());
            let pad_y = section_radius * cascade.x_axis.y.abs().max(cascade.y_axis.y.abs()).max(cascade.z_axis.y.abs());
            for chunk in self.chunks.values() {
                let Some(solid) = &chunk.solid else { continue };
                let rel = (chunk.origin - camera_pos).as_vec3();
                let center = rel + Vec3::splat(SECTION_SIZE as f32 * 0.5);
                let ndc = cascade * center.extend(1.0);
                if ndc.x.abs() > 1.0 + pad_x || ndc.y.abs() > 1.0 + pad_y {
                    continue;
                }
                device.cmd_bind_vertex_buffers(cmd, 0, &[solid.vertex.buffer], &[0]);
                device.cmd_bind_index_buffer(cmd, solid.index.buffer, 0, vk::IndexType::UINT32);
                let mvp = cascade * Mat4::from_translation(rel);
                device.cmd_push_constants(
                    cmd,
                    layout,
                    vk::ShaderStageFlags::VERTEX,
                    0,
                    as_bytes(std::slice::from_ref(&mvp)),
                );
                device.cmd_draw_indexed(cmd, solid.index_count, 1, 0, 0, 0);
            }
        }
    }

    /// Points the water pass at the (re)created opaque depth image and
    /// scene-color snapshot. Call while the device is idle.
    pub unsafe fn bind_water_inputs(
        &self,
        device: &ash::Device,
        depth: vk::ImageView,
        scene: vk::ImageView,
    ) {
        unsafe {
            let depth_info = [vk::DescriptorImageInfo::default()
                .image_view(depth)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            let scene_info = [vk::DescriptorImageInfo::default()
                .image_view(scene)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            for &set in &self.water_sets {
                device.update_descriptor_sets(
                    &[
                        vk::WriteDescriptorSet::default()
                            .dst_set(set)
                            .dst_binding(0)
                            .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                            .image_info(&depth_info),
                        vk::WriteDescriptorSet::default()
                            .dst_set(set)
                            .dst_binding(1)
                            .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                            .image_info(&scene_info),
                    ],
                    &[],
                );
            }
        }
    }

    /// Records the blended water draws (after opaques and entities).
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn record_water(
        &mut self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        view_proj: Mat4,
        camera_pos: DVec3,
        sun: Vec4,
        sky: Vec4,
        time: f32,
        fog_distance: f32,
        slot: usize,
        extent: vk::Extent2D,
        reflections: bool,
    ) {
        unsafe {
            if self.chunks.is_empty() {
                return;
            }
            // The SSR march projects reflected rays with the same camera
            // matrix the chunks use.
            let frame = WaterFrameData {
                view_proj,
                params: Vec4::new(
                    extent.width as f32,
                    extent.height as f32,
                    reflections as u32 as f32,
                    0.0,
                ),
            };
            if let Some(mapped) = self.water_uniforms[slot].1.mapped_slice_mut() {
                mapped[..size_of::<WaterFrameData>()]
                    .copy_from_slice(as_bytes(std::slice::from_ref(&frame)));
            }
            let mut bound = false;
            let frustum = frustum_planes(view_proj);
            for chunk in self.chunks.values() {
                let Some(water) = &chunk.water else { continue };
                let rel = (chunk.origin - camera_pos).as_vec3();
                if !aabb_intersects_frustum(&frustum, rel, rel + Vec3::splat(SECTION_SIZE as f32))
                {
                    continue;
                }
                if !bound {
                    device.cmd_bind_pipeline(
                        cmd,
                        vk::PipelineBindPoint::GRAPHICS,
                        self.water_pipeline,
                    );
                    device.cmd_bind_descriptor_sets(
                        cmd,
                        vk::PipelineBindPoint::GRAPHICS,
                        self.water_pipeline_layout,
                        0,
                        &[self.descriptor_set, self.water_sets[slot]],
                        &[],
                    );
                    bound = true;
                }
                device.cmd_bind_vertex_buffers(cmd, 0, &[water.vertex.buffer], &[0]);
                device.cmd_bind_index_buffer(cmd, water.index.buffer, 0, vk::IndexType::UINT32);
                // Wave phase anchors to origin mod 256 so it stays exact
                // in f32 anywhere in the world (periods divide 256).
                let origin = chunk.origin;
                let wave = Vec3::new(
                    origin.x.rem_euclid(256.0) as f32,
                    origin.y.rem_euclid(256.0) as f32,
                    origin.z.rem_euclid(256.0) as f32,
                );
                let push = WaterPush {
                    mvp: view_proj * Mat4::from_translation(rel),
                    sun,
                    sky,
                    rel: rel.extend(time),
                    wave_origin: wave.extend(fog_distance),
                };
                device.cmd_push_constants(
                    cmd,
                    self.water_pipeline_layout,
                    vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                    0,
                    as_bytes(std::slice::from_ref(&push)),
                );
                device.cmd_draw_indexed(cmd, water.index_count, 1, 0, 0, 0);
            }
        }
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    pub unsafe fn destroy(&mut self, device: &ash::Device, allocator: &mut Allocator) {
        unsafe {
            // Caller has already waited for device idle; everything is safe
            // to free immediately.
            for (_, chunk) in self.chunks.drain() {
                for part in [chunk.solid, chunk.water].into_iter().flatten() {
                    let mut part = part;
                    part.vertex.destroy(device, allocator);
                    part.index.destroy(device, allocator);
                }
            }
            for (_, mut buffer) in self.retired.drain(..) {
                buffer.destroy(device, allocator);
            }
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            device.destroy_pipeline(self.water_pipeline, None);
            device.destroy_pipeline_layout(self.water_pipeline_layout, None);
            for (buffer, alloc) in self.water_uniforms.drain(..) {
                device.destroy_buffer(buffer, None);
                let _ = allocator.free(alloc);
            }
            device.destroy_sampler(self.scene_sampler, None);
            device.destroy_descriptor_pool(self.water_depth_pool, None);
            device.destroy_descriptor_set_layout(self.water_depth_layout, None);
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

/// Frustum planes from a view-projection matrix (Gribb–Hartmann), as
/// `(normal, d)` in `Vec4`s with inward-facing normals. Unnormalized —
/// fine for sign tests. Depth range 0..1 (Vulkan).
fn frustum_planes(view_proj: Mat4) -> [Vec4; 6] {
    let r0 = view_proj.row(0);
    let r1 = view_proj.row(1);
    let r2 = view_proj.row(2);
    let r3 = view_proj.row(3);
    [
        r3 + r0, // left
        r3 - r0, // right
        r3 + r1, // bottom
        r3 - r1, // top
        r2,      // near (z >= 0)
        r3 - r2, // far
    ]
}

/// Conservative AABB-vs-frustum: false only when the box is fully outside
/// some plane.
fn aabb_intersects_frustum(planes: &[Vec4; 6], min: Vec3, max: Vec3) -> bool {
    planes.iter().all(|p| {
        // The corner furthest along the plane normal.
        let v = Vec3::new(
            if p.x >= 0.0 { max.x } else { min.x },
            if p.y >= 0.0 { max.y } else { min.y },
            if p.z >= 0.0 { max.z } else { min.z },
        );
        p.truncate().dot(v) + p.w >= 0.0
    })
}

pub(crate) fn as_bytes<T: Copy>(slice: &[T]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(slice.as_ptr().cast(), std::mem::size_of_val(slice)) }
}

pub(crate) unsafe fn create_filled_buffer(
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

/// The water pipeline: same packed vertices, but alpha-blended with the
/// depth test on and depth *writes* off (water never occludes terrain in
/// the depth buffer; later passes read opaque-only depth).
unsafe fn create_water_pipeline(
    device: &ash::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> Result<vk::Pipeline> {
    unsafe {
        let code = ash::util::read_spv(&mut std::io::Cursor::new(WATER_SPV))?;
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
        // BACK culling: the mesher emits water double-sided, so exactly
        // one winding survives from either side of the surface.
        let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::BACK)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        // The water pass has no depth attachment; the shader samples the
        // opaque depth and discards occluded fragments itself.
        let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default();
        let blend_attachment = vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
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
