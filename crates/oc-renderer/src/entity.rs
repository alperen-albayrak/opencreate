//! Entity rendering: solid tinted cuboids (placeholder until the §7.5
//! asset pipeline brings real entity models).

use anyhow::Result;
use ash::vk;
use glam::{DVec3, Mat4, Vec3};
use gpu_allocator::vulkan::Allocator;

use crate::chunk_renderer::{GpuBuffer, as_bytes, create_filled_buffer};
use crate::context::VulkanContext;

const ENTITY_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/entity.spv"));

/// One entity (or body part) to draw this frame.
#[derive(Debug, Clone, Copy)]
pub struct EntityDraw {
    /// Feet position (bottom-center), world space.
    pub position: DVec3,
    /// Facing, radians (0 = -Z).
    pub yaw: f32,
    /// Rotation around the box's local x axis (limb swing, head tilt).
    pub pitch: f32,
    /// Height of the pitch pivot above the box's feet, blocks (limbs
    /// hang from their top, heads nod from their base).
    pub pivot: f32,
    /// Box size: width (x), height (y), depth (z).
    pub size: [f32; 3],
    pub color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct EntityVertex {
    pos: [f32; 3],
    shade: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct EntityPush {
    mvp: Mat4,
    color: [f32; 4],
}

pub struct EntityRenderer {
    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    vertices: GpuBuffer,
    vertex_count: u32,
}

impl EntityRenderer {
    pub unsafe fn new(
        ctx: &VulkanContext,
        allocator: &mut Allocator,
        render_pass: vk::RenderPass,
    ) -> Result<Self> {
        unsafe {
            let verts = cuboid_vertices();
            let vertices = create_filled_buffer(
                ctx,
                allocator,
                vk::BufferUsageFlags::VERTEX_BUFFER,
                as_bytes(&verts),
                "entity cuboid",
            )?;

            let push_range = vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
                .size(size_of::<EntityPush>() as u32);
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
                vertex_count: verts.len() as u32,
            })
        }
    }

    /// Records the entity draws. Must run inside the render pass with
    /// viewport/scissor set.
    pub unsafe fn record(
        &self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        view_proj: Mat4,
        camera_pos: DVec3,
        draws: &[EntityDraw],
    ) {
        unsafe {
            if draws.is_empty() {
                return;
            }
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            device.cmd_bind_vertex_buffers(cmd, 0, &[self.vertices.buffer], &[0]);
            for draw in draws {
                // Camera-relative translation (§3), then facing, then size.
                let rel = (draw.position - camera_pos).as_vec3();
                let pivot = Vec3::new(0.0, draw.pivot, 0.0);
                let model = Mat4::from_translation(rel)
                    * Mat4::from_rotation_y(draw.yaw)
                    * Mat4::from_translation(pivot)
                    * Mat4::from_rotation_x(draw.pitch)
                    * Mat4::from_translation(-pivot)
                    * Mat4::from_scale(Vec3::from(draw.size));
                let push = EntityPush { mvp: view_proj * model, color: draw.color };
                device.cmd_push_constants(
                    cmd,
                    self.pipeline_layout,
                    vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                    0,
                    as_bytes(std::slice::from_ref(&push)),
                );
                device.cmd_draw(cmd, self.vertex_count, 1, 0, 0);
            }
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

/// A unit cuboid: x/z centered in [-0.5, 0.5], y (feet) in [0, 1], with the
/// chunk shader's per-face shading. Counter-clockwise winding outward.
fn cuboid_vertices() -> Vec<EntityVertex> {
    // Each face: (corner positions in CCW fan order, shade).
    let faces: [([Vec3; 4], f32); 6] = [
        // +Y top
        (
            [
                Vec3::new(-0.5, 1.0, 0.5),
                Vec3::new(0.5, 1.0, 0.5),
                Vec3::new(0.5, 1.0, -0.5),
                Vec3::new(-0.5, 1.0, -0.5),
            ],
            1.0,
        ),
        // -Y bottom
        (
            [
                Vec3::new(-0.5, 0.0, -0.5),
                Vec3::new(0.5, 0.0, -0.5),
                Vec3::new(0.5, 0.0, 0.5),
                Vec3::new(-0.5, 0.0, 0.5),
            ],
            0.45,
        ),
        // +Z
        (
            [
                Vec3::new(-0.5, 0.0, 0.5),
                Vec3::new(0.5, 0.0, 0.5),
                Vec3::new(0.5, 1.0, 0.5),
                Vec3::new(-0.5, 1.0, 0.5),
            ],
            0.8,
        ),
        // -Z
        (
            [
                Vec3::new(0.5, 0.0, -0.5),
                Vec3::new(-0.5, 0.0, -0.5),
                Vec3::new(-0.5, 1.0, -0.5),
                Vec3::new(0.5, 1.0, -0.5),
            ],
            0.8,
        ),
        // +X
        (
            [
                Vec3::new(0.5, 0.0, 0.5),
                Vec3::new(0.5, 0.0, -0.5),
                Vec3::new(0.5, 1.0, -0.5),
                Vec3::new(0.5, 1.0, 0.5),
            ],
            0.6,
        ),
        // -X
        (
            [
                Vec3::new(-0.5, 0.0, -0.5),
                Vec3::new(-0.5, 0.0, 0.5),
                Vec3::new(-0.5, 1.0, 0.5),
                Vec3::new(-0.5, 1.0, -0.5),
            ],
            0.6,
        ),
    ];
    let mut verts = Vec::with_capacity(36);
    for (corners, shade) in faces {
        for index in [0usize, 1, 2, 0, 2, 3] {
            verts.push(EntityVertex { pos: corners[index].to_array(), shade });
        }
    }
    verts
}

unsafe fn create_pipeline(
    device: &ash::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> Result<vk::Pipeline> {
    unsafe {
        let code = ash::util::read_spv(&mut std::io::Cursor::new(ENTITY_SPV))?;
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
            .stride(size_of::<EntityVertex>() as u32)
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
