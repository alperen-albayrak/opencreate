//! Volumetric god-rays pass (graphics roadmap VV stage 3.1): a fullscreen
//! raymarch that accumulates sun-lit in-scattering sampled against the shadow
//! cascades and additively blends it into the lit HDR color. Structurally a
//! clone of [`crate::lighting::LightingPass`], but with only the depth as a
//! G-buffer input (set 0), an additive blend, and a fatter push constant
//! carrying the fog parameters alongside `inv_view_proj`. Recorded inside the
//! lighting render pass, right after the lighting resolve (depth is a sampled
//! input there, not an attachment), reusing the Scene UBO (set 1) and the sun
//! shadow cascade set (set 2).

use anyhow::Result;
use ash::vk;
use glam::{Mat4, Vec4};

use crate::context::VulkanContext;

const VOLUMETRIC_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/volumetric.spv"));

/// Push constant for the volumetric pass — must match `VolPush` in
/// `volumetric.wgsl` (mat4 + 2×vec4 = 96 bytes, all 16-byte aligned).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VolPush {
    /// Depth -> camera-relative world (same inverse view-proj as lighting).
    pub inv_view_proj: Mat4,
    /// x: density (per block), y: mie_g, z: step count, w: max march distance.
    pub fog_a: Vec4,
    /// rgb: in-scatter tint, w: intensity.
    pub fog_b: Vec4,
}

pub struct VolumetricPass {
    descriptor_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    descriptor_set: vk::DescriptorSet,
    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
}

impl VolumetricPass {
    pub unsafe fn new(
        ctx: &VulkanContext,
        lighting_pass: vk::RenderPass,
        scene_layout: vk::DescriptorSetLayout,
        shadow_layout: vk::DescriptorSetLayout,
    ) -> Result<Self> {
        unsafe {
            let device = &ctx.device;
            // set 0, binding 0: the G-buffer depth (read via textureLoad).
            let binding = vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT);
            let descriptor_layout = device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default()
                    .bindings(std::slice::from_ref(&binding)),
                None,
            )?;
            let pool_sizes = [vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(1)];
            let descriptor_pool = device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(1)
                    .pool_sizes(&pool_sizes),
                None,
            )?;
            let descriptor_set = device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(descriptor_pool)
                    .set_layouts(std::slice::from_ref(&descriptor_layout)),
            )?[0];

            // Set 0 = depth, set 1 = Scene UBO, set 2 = shadow cascades.
            let set_layouts = [descriptor_layout, scene_layout, shadow_layout];
            let push_range = vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::FRAGMENT)
                .size(size_of::<VolPush>() as u32);
            let pipeline_layout = device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&set_layouts)
                    .push_constant_ranges(std::slice::from_ref(&push_range)),
                None,
            )?;
            let pipeline = create_pipeline(device, lighting_pass, pipeline_layout)?;
            Ok(Self {
                descriptor_layout,
                descriptor_pool,
                descriptor_set,
                pipeline_layout,
                pipeline,
            })
        }
    }

    /// Points the pass at the (re)created depth view. Call while idle.
    pub unsafe fn bind_input(&self, device: &ash::Device, depth: vk::ImageView) {
        unsafe {
            let info = vk::DescriptorImageInfo::default()
                .image_view(depth)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
            let write = vk::WriteDescriptorSet::default()
                .dst_set(self.descriptor_set)
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .image_info(std::slice::from_ref(&info));
            device.update_descriptor_sets(std::slice::from_ref(&write), &[]);
        }
    }

    /// Records the fullscreen raymarch. Must run inside the lighting render
    /// pass (after the lighting resolve), with the world viewport/scissor set.
    pub unsafe fn record(
        &self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        scene_set: vk::DescriptorSet,
        shadow_set: vk::DescriptorSet,
        push: VolPush,
    ) {
        unsafe {
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout,
                0,
                &[self.descriptor_set, scene_set, shadow_set],
                &[],
            );
            device.cmd_push_constants(
                cmd,
                self.pipeline_layout,
                vk::ShaderStageFlags::FRAGMENT,
                0,
                crate::chunk_renderer::as_bytes(std::slice::from_ref(&push)),
            );
            device.cmd_draw(cmd, 3, 1, 0, 0);
        }
    }

    pub unsafe fn destroy(&mut self, device: &ash::Device) {
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            device.destroy_descriptor_pool(self.descriptor_pool, None);
            device.destroy_descriptor_set_layout(self.descriptor_layout, None);
        }
    }
}

unsafe fn create_pipeline(
    device: &ash::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> Result<vk::Pipeline> {
    unsafe {
        let code = ash::util::read_spv(&mut std::io::Cursor::new(VOLUMETRIC_SPV))?;
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
        let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default();
        // Additive: in-scattering adds onto the lit color (the dst).
        let blend_attachment = vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::ONE)
            .dst_color_blend_factor(vk::BlendFactor::ONE)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ONE)
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
