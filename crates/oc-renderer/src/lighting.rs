//! Deferred lighting pass (graphics roadmap Stage E): a fullscreen triangle
//! resolves the G-buffer (written by the geometry pass) into the HDR color,
//! reading the per-frame Scene UBO. Mirrors `TonemapPass`'s structure;
//! `pbr.wgsl` does the shading (sky/sun/block light, AO, fog — parity now,
//! the seam later PBR/SSAO/many-light features plug into).

use anyhow::Result;
use ash::vk;

use crate::context::VulkanContext;

const PBR_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/pbr.spv"));

pub struct LightingPass {
    descriptor_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    descriptor_set: vk::DescriptorSet,
    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
}

impl LightingPass {
    pub unsafe fn new(
        ctx: &VulkanContext,
        lighting_pass: vk::RenderPass,
        scene_layout: vk::DescriptorSetLayout,
        shadow_layout: vk::DescriptorSetLayout,
    ) -> Result<Self> {
        unsafe {
            let device = &ctx.device;
            // group(0): GB0, GB1, GB2, depth — all read via textureLoad, so a
            // sampled image each (no sampler needed at 1:1 resolution).
            let bindings: Vec<_> = (0..4u32)
                .map(|b| {
                    vk::DescriptorSetLayoutBinding::default()
                        .binding(b)
                        .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                        .descriptor_count(1)
                        .stage_flags(vk::ShaderStageFlags::FRAGMENT)
                })
                .collect();
            let descriptor_layout = device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )?;
            let pool_sizes = [vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(4)];
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

            // Set 0 = G-buffer, set 1 = the shared Scene UBO, set 2 = shadow
            // cascades. The push constant carries the inverse view-projection
            // (depth -> camera-relative world for the cascade lookup).
            let set_layouts = [descriptor_layout, scene_layout, shadow_layout];
            let push_range = vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::FRAGMENT)
                .size(size_of::<glam::Mat4>() as u32);
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

    /// Points the pass at the (re)created G-buffer + depth views. Call while
    /// the device is idle (target recreation already requires that).
    pub unsafe fn bind_input(
        &self,
        device: &ash::Device,
        gb0: vk::ImageView,
        gb1: vk::ImageView,
        gb2: vk::ImageView,
        depth: vk::ImageView,
    ) {
        unsafe {
            let views = [gb0, gb1, gb2, depth];
            let infos: Vec<_> = views
                .iter()
                .map(|&v| {
                    vk::DescriptorImageInfo::default()
                        .image_view(v)
                        .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                })
                .collect();
            let writes: Vec<_> = (0..4u32)
                .map(|b| {
                    vk::WriteDescriptorSet::default()
                        .dst_set(self.descriptor_set)
                        .dst_binding(b)
                        .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                        .image_info(std::slice::from_ref(&infos[b as usize]))
                })
                .collect();
            device.update_descriptor_sets(&writes, &[]);
        }
    }

    /// Records the fullscreen lighting resolve. Must run inside the lighting
    /// render pass with the world viewport/scissor set.
    pub unsafe fn record(
        &self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        scene_set: vk::DescriptorSet,
        shadow_set: vk::DescriptorSet,
        inv_view_proj: glam::Mat4,
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
                crate::chunk_renderer::as_bytes(std::slice::from_ref(&inv_view_proj)),
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
        let code = ash::util::read_spv(&mut std::io::Cursor::new(PBR_SPV))?;
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
        // Fullscreen: no depth test, writes every covered pixel.
        let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default();
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
