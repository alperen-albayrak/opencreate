//! Dynamic point lights (graphics roadmap P3): emissive blocks (torches, lava,
//! lamps) that cast real coloured light + specular glints, computed per-pixel in
//! the deferred lighting pass. The active set is uploaded each frame into a UBO
//! (set 3 of the lighting pipeline) — positions are **camera-relative**, the
//! same space `pbr.wgsl` rebuilds from depth. Mirrors [`crate::scene::SceneUbo`]:
//! one buffer + descriptor set per frame-in-flight so the CPU never writes a
//! buffer the GPU may still read.
//!
//! Clustering and per-light shadows are deferred — this is a flat, distance-
//! culled list capped at [`MAX_POINT_LIGHTS`], looped in the shader.

use ash::vk;
use glam::Vec4;
use gpu_allocator::MemoryLocation;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme, Allocator};

use crate::chunk_renderer::as_bytes;
use crate::context::VulkanContext;

/// Max active point lights per frame (the shader loops this; the CPU culls to
/// the nearest this many). Keep in sync with the array size in `pbr.wgsl`.
pub const MAX_POINT_LIGHTS: usize = 64;

/// One point light. `pos_radius`: xyz camera-relative position, w radius (blocks
/// beyond which it contributes nothing). `color_intensity`: rgb colour, w peak
/// intensity. Mirrors the WGSL `PointLight`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PointLight {
    pub pos_radius: Vec4,
    pub color_intensity: Vec4,
}

impl PointLight {
    pub const ZERO: PointLight =
        PointLight { pos_radius: Vec4::ZERO, color_intensity: Vec4::ZERO };
}

/// The UBO payload: a count (in `header.x`) + the fixed-size light array. std140
/// agrees with the WGSL `PointLights` (vec4 header, then a 2×vec4 array).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PointLightData {
    /// x: active light count (0..=MAX_POINT_LIGHTS); yzw reserved.
    pub header: Vec4,
    pub lights: [PointLight; MAX_POINT_LIGHTS],
}

impl PointLightData {
    /// Builds the payload from a slice of active lights (truncated to the cap).
    pub fn new(active: &[PointLight]) -> Self {
        let n = active.len().min(MAX_POINT_LIGHTS);
        let mut lights = [PointLight::ZERO; MAX_POINT_LIGHTS];
        lights[..n].copy_from_slice(&active[..n]);
        Self { header: Vec4::new(n as f32, 0.0, 0.0, 0.0), lights }
    }
}

/// Owns the per-frame point-light uniform buffers and their descriptor sets.
pub struct PointLightUbo {
    layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    sets: Vec<vk::DescriptorSet>,
    buffers: Vec<(vk::Buffer, Allocation)>,
}

impl PointLightUbo {
    pub unsafe fn new(
        ctx: &VulkanContext,
        allocator: &mut Allocator,
        frames: usize,
    ) -> anyhow::Result<Self> {
        unsafe {
            let device = &ctx.device;
            let binding = vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT);
            let layout = device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default()
                    .bindings(std::slice::from_ref(&binding)),
                None,
            )?;
            let pool_size = vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(frames as u32);
            let pool = device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(frames as u32)
                    .pool_sizes(std::slice::from_ref(&pool_size)),
                None,
            )?;
            let mut sets = Vec::with_capacity(frames);
            let mut buffers = Vec::with_capacity(frames);
            for i in 0..frames {
                let set = device.allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(pool)
                        .set_layouts(std::slice::from_ref(&layout)),
                )?[0];
                let buffer = device.create_buffer(
                    &vk::BufferCreateInfo::default()
                        .size(size_of::<PointLightData>() as u64)
                        .usage(vk::BufferUsageFlags::UNIFORM_BUFFER),
                    None,
                )?;
                let requirements = device.get_buffer_memory_requirements(buffer);
                let alloc = allocator.allocate(&AllocationCreateDesc {
                    name: &format!("point-light ubo {i}"),
                    requirements,
                    location: MemoryLocation::CpuToGpu,
                    linear: true,
                    allocation_scheme: AllocationScheme::GpuAllocatorManaged,
                })?;
                device.bind_buffer_memory(buffer, alloc.memory(), alloc.offset())?;
                let buffer_info = [vk::DescriptorBufferInfo::default()
                    .buffer(buffer)
                    .range(size_of::<PointLightData>() as u64)];
                device.update_descriptor_sets(
                    &[vk::WriteDescriptorSet::default()
                        .dst_set(set)
                        .dst_binding(0)
                        .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                        .buffer_info(&buffer_info)],
                    &[],
                );
                sets.push(set);
                buffers.push((buffer, alloc));
            }
            Ok(Self { layout, pool, sets, buffers })
        }
    }

    /// The descriptor set layout (set 3 of the lighting pipeline).
    pub fn layout(&self) -> vk::DescriptorSetLayout {
        self.layout
    }

    /// This frame slot's descriptor set, to bind in the lighting pass.
    pub fn set(&self, slot: usize) -> vk::DescriptorSet {
        self.sets[slot]
    }

    /// Uploads this frame's light set into `slot`'s buffer (host-visible).
    pub fn update(&mut self, slot: usize, data: &PointLightData) {
        if let Some(mapped) = self.buffers[slot].1.mapped_slice_mut() {
            mapped[..size_of::<PointLightData>()]
                .copy_from_slice(as_bytes(std::slice::from_ref(data)));
        }
    }

    pub unsafe fn destroy(&mut self, device: &ash::Device, allocator: &mut Allocator) {
        unsafe {
            for (buffer, alloc) in self.buffers.drain(..) {
                device.destroy_buffer(buffer, None);
                let _ = allocator.free(alloc);
            }
            device.destroy_descriptor_pool(self.pool, None);
            device.destroy_descriptor_set_layout(self.layout, None);
        }
    }
}
