//! Draws the far-terrain LOD tiles (coarse colored heightmaps generated
//! client-side from the seed). Rendered in the world pass after the
//! chunks: depth testing keeps detailed terrain on top, and the ring
//! shows only where no real geometry exists — the horizon.

use std::collections::HashMap;

use anyhow::Result;
use ash::vk;
use glam::{DVec3, Mat4, Vec3, Vec4};
use gpu_allocator::vulkan::Allocator;

use crate::FRAMES_IN_FLIGHT;
use crate::chunk_renderer::{GpuBuffer, as_bytes, create_filled_buffer};
use crate::context::VulkanContext;

const FAR_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/far.spv"));

/// One vertex of a far tile, in tile-local coordinates.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FarVertex {
    pub position: [f32; 3],
    pub color: [f32; 4],
}

/// A generated far tile, ready for upload.
pub struct FarTile {
    /// World position of the tile's minimum corner (y unused).
    pub origin: DVec3,
    pub vertices: Vec<FarVertex>,
    pub indices: Vec<u32>,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct FarPush {
    mvp: Mat4,
    fog: Vec4,
    params: Vec4,
}

struct TileGpu {
    vertex: GpuBuffer,
    index: GpuBuffer,
    index_count: u32,
    origin: DVec3,
}

pub struct FarRenderer {
    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    tiles: HashMap<(i32, i32), TileGpu>,
    retired: Vec<(u64, GpuBuffer)>,
}

impl FarRenderer {
    pub unsafe fn new(ctx: &VulkanContext, render_pass: vk::RenderPass) -> Result<Self> {
        unsafe {
            let push_range = vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
                .size(size_of::<FarPush>() as u32);
            let pipeline_layout = ctx.device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .push_constant_ranges(std::slice::from_ref(&push_range)),
                None,
            )?;
            let pipeline = create_pipeline(&ctx.device, render_pass, pipeline_layout)?;
            Ok(Self {
                pipeline_layout,
                pipeline,
                tiles: HashMap::new(),
                retired: Vec::new(),
            })
        }
    }

    pub unsafe fn set_tile(
        &mut self,
        ctx: &VulkanContext,
        allocator: &mut Allocator,
        frame: u64,
        key: (i32, i32),
        tile: &FarTile,
    ) -> Result<()> {
        unsafe {
            let vertex = create_filled_buffer(
                ctx,
                allocator,
                vk::BufferUsageFlags::VERTEX_BUFFER,
                as_bytes(&tile.vertices),
                "far tile vertices",
            )?;
            let index = create_filled_buffer(
                ctx,
                allocator,
                vk::BufferUsageFlags::INDEX_BUFFER,
                as_bytes(&tile.indices),
                "far tile indices",
            )?;
            if let Some(old) = self.tiles.insert(
                key,
                TileGpu {
                    vertex,
                    index,
                    index_count: tile.indices.len() as u32,
                    origin: tile.origin,
                },
            ) {
                self.retired.push((frame, old.vertex));
                self.retired.push((frame, old.index));
            }
            Ok(())
        }
    }

    pub fn remove_tile(&mut self, frame: u64, key: (i32, i32)) {
        if let Some(old) = self.tiles.remove(&key) {
            self.retired.push((frame, old.vertex));
            self.retired.push((frame, old.index));
        }
    }

    /// Frees retired buffers that are provably out of flight (same scheme
    /// as the chunk renderer).
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

    /// Records the tile draws; runs in the world pass after the chunks.
    pub unsafe fn record(
        &self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        view_proj: Mat4,
        camera_pos: DVec3,
        fog: Vec4,
        daylight: f32,
    ) {
        unsafe {
            if self.tiles.is_empty() {
                return;
            }
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            for tile in self.tiles.values() {
                let rel = (tile.origin - camera_pos).as_vec3();
                // Coarse cull: skip tiles fully behind the camera plane.
                // (256-block tiles; the GPU clips the rest fine.)
                let center = rel + Vec3::new(128.0, 0.0, 128.0);
                let clip = view_proj * center.extend(1.0);
                if clip.w < -300.0 {
                    continue;
                }
                device.cmd_bind_vertex_buffers(cmd, 0, &[tile.vertex.buffer], &[0]);
                device.cmd_bind_index_buffer(cmd, tile.index.buffer, 0, vk::IndexType::UINT32);
                let push = FarPush {
                    mvp: view_proj * Mat4::from_translation(rel),
                    fog,
                    params: Vec4::new(daylight, 0.0, 0.0, 0.0),
                };
                device.cmd_push_constants(
                    cmd,
                    self.pipeline_layout,
                    vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                    0,
                    as_bytes(std::slice::from_ref(&push)),
                );
                device.cmd_draw_indexed(cmd, tile.index_count, 1, 0, 0, 0);
            }
        }
    }

    pub unsafe fn destroy(&mut self, device: &ash::Device, allocator: &mut Allocator) {
        unsafe {
            for (_, tile) in self.tiles.drain() {
                let mut tile = tile;
                tile.vertex.destroy(device, allocator);
                tile.index.destroy(device, allocator);
            }
            for (_, mut buffer) in self.retired.drain(..) {
                buffer.destroy(device, allocator);
            }
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
        }
    }
}

unsafe fn create_pipeline(
    device: &ash::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> Result<vk::Pipeline> {
    unsafe {
        let code = ash::util::read_spv(&mut std::io::Cursor::new(FAR_SPV))?;
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
            .stride(size_of::<FarVertex>() as u32)
            .input_rate(vk::VertexInputRate::VERTEX);
        let attributes = [
            vk::VertexInputAttributeDescription::default()
                .location(0)
                .format(vk::Format::R32G32B32_SFLOAT),
            vk::VertexInputAttributeDescription::default()
                .location(1)
                .format(vk::Format::R32G32B32A32_SFLOAT)
                .offset(12),
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
