//! The sky dome (graphics roadmap stage C): a fullscreen triangle drawn
//! at the far plane after the opaques — only pixels nothing rendered to
//! get sky-shaded. Lives in the world (HDR) render pass.

use anyhow::Result;
use ash::vk;
use glam::{Mat4, Vec4};

use crate::chunk_renderer::as_bytes;
use crate::context::VulkanContext;

const SKY_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/sky.spv"));

/// Push constants; must match `sky.wgsl`. Exactly 128 bytes — the
/// minimum-spec push budget; the scalars ride in the colors' w slots.
#[repr(C)]
#[derive(Clone, Copy)]
struct SkyPush {
    inv_view_proj: Mat4,
    /// xyz: unscaled sun direction; w: daylight.
    sun: Vec4,
    /// rgb: toward-sun horizon; w: celestial angle, radians.
    horizon: Vec4,
    /// rgb: anti-sun horizon; w: moon phase 0..1.
    away: Vec4,
    /// rgb: zenith; w: star visibility 0..1.
    zenith: Vec4,
}

pub struct SkyPass {
    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
}

impl SkyPass {
    pub unsafe fn new(ctx: &VulkanContext, render_pass: vk::RenderPass) -> Result<Self> {
        unsafe {
            let device = &ctx.device;
            let push_range = vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
                .size(size_of::<SkyPush>() as u32);
            let pipeline_layout = device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .push_constant_ranges(std::slice::from_ref(&push_range)),
                None,
            )?;

            let code = ash::util::read_spv(&mut std::io::Cursor::new(SKY_SPV))?;
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
            // The dome sits exactly on the far plane: LESS_EQUAL passes
            // where the depth clear (1.0) survived, fails on geometry.
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
                .layout(pipeline_layout)
                .render_pass(render_pass);
            let pipeline = device
                .create_graphics_pipelines(vk::PipelineCache::null(), &[info], None)
                .map_err(|(_, e)| e)?[0];
            device.destroy_shader_module(module, None);

            Ok(Self { pipeline_layout, pipeline })
        }
    }

    /// Records the dome. Must run inside the world pass, after opaques.
    pub unsafe fn record(
        &self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        view_proj: Mat4,
        sun: Vec4,
        horizon: Vec4,
        away: Vec4,
        zenith: Vec4,
    ) {
        unsafe {
            let push = SkyPush {
                inv_view_proj: view_proj.inverse(),
                sun,
                horizon,
                away,
                zenith,
            };
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            device.cmd_push_constants(
                cmd,
                self.pipeline_layout,
                vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                0,
                as_bytes(std::slice::from_ref(&push)),
            );
            device.cmd_draw(cmd, 3, 1, 0, 0);
        }
    }

    pub unsafe fn destroy(&mut self, device: &ash::Device) {
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
        }
    }
}
