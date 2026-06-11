//! Wireframe outline around the targeted block.

use anyhow::Result;
use ash::vk;
use glam::{DVec3, IVec3, Mat4, Vec3};
use gpu_allocator::vulkan::Allocator;

use crate::chunk_renderer::{GpuBuffer, as_bytes, create_filled_buffer};
use crate::context::VulkanContext;

const OUTLINE_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/outline.spv"));

/// Slight inflation so the lines sit just outside the block faces instead of
/// z-fighting with them.
const INFLATE: f32 = 0.004;

pub struct OutlineRenderer {
    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    vertices: GpuBuffer,
    vertex_count: u32,
}

impl OutlineRenderer {
    pub unsafe fn new(
        ctx: &VulkanContext,
        allocator: &mut Allocator,
        render_pass: vk::RenderPass,
    ) -> Result<Self> {
        unsafe {
            let device = &ctx.device;

            let verts = cube_edges();
            let vertices = create_filled_buffer(
                ctx,
                allocator,
                vk::BufferUsageFlags::VERTEX_BUFFER,
                as_bytes(&verts),
                "block outline",
            )?;

            let push_range = vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::VERTEX)
                .size(size_of::<Mat4>() as u32);
            let pipeline_layout = device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .push_constant_ranges(std::slice::from_ref(&push_range)),
                None,
            )?;
            let pipeline = create_pipeline(device, render_pass, pipeline_layout)?;

            Ok(Self {
                pipeline_layout,
                pipeline,
                vertices,
                vertex_count: verts.len() as u32,
            })
        }
    }

    /// Records the outline draw for the block at `block` (world coords).
    /// Must be called inside the render pass after viewport/scissor are set.
    pub unsafe fn record(
        &self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        view_proj: Mat4,
        camera_pos: DVec3,
        block: IVec3,
    ) {
        unsafe {
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            device.cmd_bind_vertex_buffers(cmd, 0, &[self.vertices.buffer], &[0]);
            let rel = (block.as_dvec3() - camera_pos).as_vec3();
            let mvp = view_proj * Mat4::from_translation(rel);
            device.cmd_push_constants(
                cmd,
                self.pipeline_layout,
                vk::ShaderStageFlags::VERTEX,
                0,
                as_bytes(std::slice::from_ref(&mvp)),
            );
            device.cmd_draw(cmd, self.vertex_count, 1, 0, 0);
        }
    }

    pub unsafe fn destroy(&mut self, device: &ash::Device, allocator: &mut Allocator) {
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            self.vertices.destroy(device, allocator);
        }
    }
}

/// The 12 edges of a slightly inflated unit cube as a line list.
fn cube_edges() -> Vec<Vec3> {
    let lo = -INFLATE;
    let hi = 1.0 + INFLATE;
    let corner = |x: u32, y: u32, z: u32| {
        Vec3::new(
            if x == 0 { lo } else { hi },
            if y == 0 { lo } else { hi },
            if z == 0 { lo } else { hi },
        )
    };
    let mut verts = Vec::with_capacity(24);
    // For each axis, the 4 edges running along it.
    for (a, b) in [(0u32, 0u32), (0, 1), (1, 0), (1, 1)] {
        verts.push(corner(0, a, b));
        verts.push(corner(1, a, b));
        verts.push(corner(a, 0, b));
        verts.push(corner(a, 1, b));
        verts.push(corner(a, b, 0));
        verts.push(corner(a, b, 1));
    }
    verts
}

unsafe fn create_pipeline(
    device: &ash::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> Result<vk::Pipeline> {
    unsafe {
        let code = ash::util::read_spv(&mut std::io::Cursor::new(OUTLINE_SPV))?;
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
            .stride(size_of::<Vec3>() as u32)
            .input_rate(vk::VertexInputRate::VERTEX);
        let attribute = vk::VertexInputAttributeDescription::default()
            .location(0)
            .format(vk::Format::R32G32B32_SFLOAT);
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(std::slice::from_ref(&binding))
            .vertex_attribute_descriptions(std::slice::from_ref(&attribute));

        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::LINE_LIST);
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
        let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(true)
            .depth_write_enable(false)
            .depth_compare_op(vk::CompareOp::LESS_OR_EQUAL);
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
