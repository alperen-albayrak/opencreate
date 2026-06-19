//! Per-frame Scene/Environment UBO (descriptor set bound by the world passes).
//!
//! The sun/fog/sky/time values are identical for every draw in a frame, so
//! rather than copy them into each pass's push constants (chunk, water, sky,
//! entity, clouds, far), they live in one uniform buffer updated once per frame.
//! Push constants then carry only genuinely per-draw data (the MVP, chunk
//! origin). One buffer + descriptor set per frame-in-flight, mirroring the water
//! SSR UBO, so the CPU never writes a buffer the GPU may still be reading.
//!
//! Layout is std140: a sequence of `vec4`s (each 16-byte aligned), so the Rust
//! [`SceneData`] and the WGSL `Scene` struct agree byte-for-byte.

use ash::vk;
use glam::Vec4;
use gpu_allocator::MemoryLocation;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme, Allocator};

use crate::chunk_renderer::as_bytes;
use crate::context::VulkanContext;

/// Per-frame scene/environment data shared by all world passes. Mirror of the
/// WGSL `Scene` struct; all fields are `vec4` for std140 agreement.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SceneData {
    /// xyz: direction toward the sun (scaled by daylight); w: ambient level.
    pub sun: Vec4,
    /// rgb: distance-fog (horizon) color; w: fog saturation distance (blocks).
    pub fog: Vec4,
    /// rgb: toward-sun horizon color; w: celestial rotation angle (radians).
    pub sky_horizon: Vec4,
    /// rgb: zenith color; w: star visibility 0..1.
    pub sky_zenith: Vec4,
    /// rgb: anti-sun horizon color; w: moon phase 0..1.
    pub sky_away: Vec4,
    /// xyz: unscaled sun direction; w: daylight 0..1.
    pub sky_sun: Vec4,
    /// x: time (seconds); y: base ambient floor; z: camera world Y (for the
    /// geothermal glow's absolute-depth reconstruction); w: reserved.
    pub params: Vec4,
    /// Active dimension's geothermal profile: x = surface °C, y = gradient
    /// °C/block, z = core °C, w = sea level. Drives the deep-rock blackbody
    /// glow in the lighting pass (a coreless/airless world sets a cold,
    /// no-glow sentinel).
    pub thermal: Vec4,
}

/// Owns the per-frame scene uniform buffers and their descriptor sets.
pub struct SceneUbo {
    layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    sets: Vec<vk::DescriptorSet>,
    buffers: Vec<(vk::Buffer, Allocation)>,
}

impl SceneUbo {
    /// Creates `frames` buffers + descriptor sets (one per frame-in-flight).
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
                .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT);
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
                        .size(size_of::<SceneData>() as u64)
                        .usage(vk::BufferUsageFlags::UNIFORM_BUFFER),
                    None,
                )?;
                let requirements = device.get_buffer_memory_requirements(buffer);
                let alloc = allocator.allocate(&AllocationCreateDesc {
                    name: &format!("scene ubo {i}"),
                    requirements,
                    location: MemoryLocation::CpuToGpu,
                    linear: true,
                    allocation_scheme: AllocationScheme::GpuAllocatorManaged,
                })?;
                device.bind_buffer_memory(buffer, alloc.memory(), alloc.offset())?;
                let buffer_info = [vk::DescriptorBufferInfo::default()
                    .buffer(buffer)
                    .range(size_of::<SceneData>() as u64)];
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

    /// The descriptor set layout, to append to each world pipeline layout.
    pub fn layout(&self) -> vk::DescriptorSetLayout {
        self.layout
    }

    /// This frame slot's descriptor set, to bind in each world pass.
    pub fn set(&self, slot: usize) -> vk::DescriptorSet {
        self.sets[slot]
    }

    /// Uploads this frame's scene data into `slot`'s buffer (host-visible).
    pub fn update(&mut self, slot: usize, data: &SceneData) {
        if let Some(mapped) = self.buffers[slot].1.mapped_slice_mut() {
            mapped[..size_of::<SceneData>()]
                .copy_from_slice(as_bytes(std::slice::from_ref(data)));
        }
    }

    /// Frees the buffers, pool, and layout. Call once the GPU is idle.
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
