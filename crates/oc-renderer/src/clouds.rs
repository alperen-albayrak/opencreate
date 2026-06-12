//! Blocky cloud layer (graphics roadmap stage C2): the classic flat white
//! slabs at cloud height. A tileable hash-grid of 12-block cells extrudes
//! into 4-block-thick boxes meshed once at startup; the layer drifts with
//! time and tiles 3x3 around the camera. Tinted by the day cycle (warm at
//! dusk, dark at night), drawn blended after the sky dome.

use anyhow::Result;
use ash::vk;
use glam::{DVec3, Mat4, Vec4};
use gpu_allocator::vulkan::Allocator;

use crate::chunk_renderer::{GpuBuffer, as_bytes, create_filled_buffer};
use crate::context::VulkanContext;

const CLOUD_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/cloud.spv"));

/// Altitude of the cloud layer's underside, in blocks.
pub const CLOUD_ALTITUDE: f64 = 192.0;
/// One pattern cell, blocks.
const CELL: f32 = 12.0;
/// Cells per tile edge; the pattern wraps at this period.
const GRID: i32 = 32;
/// Tile edge length, blocks.
pub const TILE: f32 = CELL * GRID as f32;
/// Slab thickness, blocks.
const THICK: f32 = 4.0;
/// Wind drift, blocks per second (+X, slightly +Z).
const WIND: (f32, f32) = (0.55, 0.1);

#[repr(C)]
#[derive(Clone, Copy)]
struct CloudVertex {
    pos: [f32; 3],
    shade: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CloudPush {
    mvp: Mat4,
    /// rgb: cloud color for the moment of day; a: layer opacity.
    color: Vec4,
}

/// Whether the wrapped cell (i, j) holds a cloud. Pure and tileable:
/// a hash smoothed over the 3x3 neighborhood gives clumpy blobs.
fn cell_present(i: i32, j: i32) -> bool {
    let h = |x: i32, z: i32| -> f32 {
        let (x, z) = (x.rem_euclid(GRID), z.rem_euclid(GRID));
        let mut v = (x as u64)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add((z as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F))
            ^ 0xC10D_5EED;
        v = (v ^ (v >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        ((v >> 40) & 0xFFFF) as f32 / 65535.0
    };
    let mut sum = 0.0;
    for dz in -1..=1 {
        for dx in -1..=1 {
            sum += h(i + dx, j + dz);
        }
    }
    sum / 9.0 > 0.52
}

/// Builds the tile mesh: a box per present cell, with side faces only at
/// pattern boundaries (wrapped, so the tiling is seamless).
fn build_mesh() -> (Vec<CloudVertex>, Vec<u32>) {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let mut quad = |corners: [[f32; 3]; 4], shade: f32| {
        let base = vertices.len() as u32;
        for pos in corners {
            vertices.push(CloudVertex { pos, shade });
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 1, base + 3]);
    };

    for j in 0..GRID {
        for i in 0..GRID {
            if !cell_present(i, j) {
                continue;
            }
            let (x0, z0) = (i as f32 * CELL, j as f32 * CELL);
            let (x1, z1) = (x0 + CELL, z0 + CELL);
            let (y0, y1) = (0.0, THICK);
            // Top (+Y), seen from below too (clouds are double-sided via
            // disabled culling in the pipeline).
            quad([[x0, y1, z1], [x1, y1, z1], [x0, y1, z0], [x1, y1, z0]], 1.0);
            quad([[x0, y0, z0], [x1, y0, z0], [x0, y0, z1], [x1, y0, z1]], 0.72);
            if !cell_present(i, j + 1) {
                quad([[x0, y0, z1], [x1, y0, z1], [x0, y1, z1], [x1, y1, z1]], 0.86);
            }
            if !cell_present(i, j - 1) {
                quad([[x1, y0, z0], [x0, y0, z0], [x1, y1, z0], [x0, y1, z0]], 0.86);
            }
            if !cell_present(i + 1, j) {
                quad([[x1, y0, z1], [x1, y0, z0], [x1, y1, z1], [x1, y1, z0]], 0.8);
            }
            if !cell_present(i - 1, j) {
                quad([[x0, y0, z0], [x0, y0, z1], [x0, y1, z0], [x0, y1, z1]], 0.8);
            }
        }
    }
    (vertices, indices)
}

pub struct CloudLayer {
    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    vertices: GpuBuffer,
    indices: GpuBuffer,
    index_count: u32,
}

impl CloudLayer {
    pub unsafe fn new(
        ctx: &VulkanContext,
        allocator: &mut Allocator,
        render_pass: vk::RenderPass,
    ) -> Result<Self> {
        unsafe {
            let (verts, inds) = build_mesh();
            let vertices = create_filled_buffer(
                ctx,
                allocator,
                vk::BufferUsageFlags::VERTEX_BUFFER,
                as_bytes(&verts),
                "cloud vertices",
            )?;
            let indices = create_filled_buffer(
                ctx,
                allocator,
                vk::BufferUsageFlags::INDEX_BUFFER,
                as_bytes(&inds),
                "cloud indices",
            )?;

            // The fragment stage reads the color too.
            let push_range = vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
                .size(size_of::<CloudPush>() as u32);
            let pipeline_layout = ctx.device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .push_constant_ranges(std::slice::from_ref(&push_range)),
                None,
            )?;
            let pipeline = create_pipeline(&ctx.device, render_pass, pipeline_layout)?;

            Ok(Self {
                pipeline_layout,
                pipeline,
                vertices,
                indices,
                index_count: inds.len() as u32,
            })
        }
    }

    /// Records the 3x3 cloud tiles around the camera, drifting with time.
    /// Must run inside the world pass, after the sky dome.
    pub unsafe fn record(
        &self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        view_proj: Mat4,
        camera_pos: DVec3,
        time: f32,
        color: Vec4,
    ) {
        unsafe {
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            device.cmd_bind_vertex_buffers(cmd, 0, &[self.vertices.buffer], &[0]);
            device.cmd_bind_index_buffer(cmd, self.indices.buffer, 0, vk::IndexType::UINT32);

            // The layer's world offset drifts with the wind and wraps per
            // tile, so anchoring near the camera stays fp32-exact.
            let drift_x = (time * WIND.0) as f64;
            let drift_z = (time * WIND.1) as f64;
            let base_x = ((camera_pos.x - drift_x) / TILE as f64).floor() * TILE as f64;
            let base_z = ((camera_pos.z - drift_z) / TILE as f64).floor() * TILE as f64;
            for tz in -1..=1 {
                for tx in -1..=1 {
                    let origin = DVec3::new(
                        base_x + drift_x + tx as f64 * TILE as f64,
                        CLOUD_ALTITUDE,
                        base_z + drift_z + tz as f64 * TILE as f64,
                    );
                    let rel = (origin - camera_pos).as_vec3();
                    let push = CloudPush {
                        mvp: view_proj * Mat4::from_translation(rel),
                        color,
                    };
                    device.cmd_push_constants(
                        cmd,
                        self.pipeline_layout,
                        vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                        0,
                        as_bytes(std::slice::from_ref(&push)),
                    );
                    device.cmd_draw_indexed(cmd, self.index_count, 1, 0, 0, 0);
                }
            }
        }
    }

    pub unsafe fn destroy(&mut self, device: &ash::Device, allocator: &mut Allocator) {
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            self.vertices.destroy(device, allocator);
            self.indices.destroy(device, allocator);
        }
    }
}

unsafe fn create_pipeline(
    device: &ash::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> Result<vk::Pipeline> {
    unsafe {
        let code = ash::util::read_spv(&mut std::io::Cursor::new(CLOUD_SPV))?;
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
            .stride(size_of::<CloudVertex>() as u32)
            .input_rate(vk::VertexInputRate::VERTEX);
        let attributes = [
            vk::VertexInputAttributeDescription::default()
                .location(0)
                .format(vk::Format::R32G32B32_SFLOAT),
            vk::VertexInputAttributeDescription::default()
                .location(1)
                .format(vk::Format::R32_SFLOAT)
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
        // No culling: the layer reads correctly from above and below.
        let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        // Depth-tested so mountains occlude clouds, but not written:
        // clouds never hide terrain from later passes.
        let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(true)
            .depth_write_enable(false)
            .depth_compare_op(vk::CompareOp::LESS);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_tiles_seamlessly() {
        for j in -2..GRID + 2 {
            for i in -2..GRID + 2 {
                assert_eq!(cell_present(i, j), cell_present(i + GRID, j));
                assert_eq!(cell_present(i, j), cell_present(i, j + GRID));
            }
        }
    }

    #[test]
    fn mesh_has_reasonable_coverage_and_closed_sides() {
        let (vertices, indices) = build_mesh();
        assert!(!vertices.is_empty(), "some clouds must exist");
        assert_eq!(indices.len() % 6, 0, "quads only");
        // Coverage sanity: not empty sky, not overcast.
        let cells = (0..GRID * GRID)
            .filter(|n| cell_present(n % GRID, n / GRID))
            .count();
        let coverage = cells as f32 / (GRID * GRID) as f32;
        assert!(
            (0.1..0.8).contains(&coverage),
            "cloud coverage should be moderate: {coverage}"
        );
        // Every present cell contributes at least top + bottom.
        assert!(vertices.len() >= cells * 8);
    }
}
