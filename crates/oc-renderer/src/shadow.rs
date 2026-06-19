//! Cascaded sun shadows (graphics roadmap stage D): solid chunks re-render
//! depth-only from the sun into three 2048² cascades ringing the camera
//! (24 / 72 / 200 blocks). The chunk shader picks a cascade per fragment,
//! PCF-filters a comparison sample, and darkens only the sun-diffuse term,
//! so block light and ambient still fill shadows naturally. Cascade
//! origins snap to shadow texels — panning the camera never shimmers the
//! shadow edges — and the whole effect fades out through twilight.

use anyhow::Result;
use ash::vk;
use glam::{DVec3, Mat4, Vec3, Vec4};
use gpu_allocator::MemoryLocation;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme, Allocator};

use crate::chunk_renderer::as_bytes;
use crate::context::VulkanContext;

const SHADOW_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/shadow.spv"));

/// Shadow map edge, per cascade.
pub const MAP_SIZE: u32 = 2048;
/// Cascade radii in blocks: a fragment picks the first that contains it.
pub const RADII: [f32; 3] = [24.0, 72.0, 200.0];
/// Light-space depth half-range: how far above/below the box casters count.
const DEPTH_RANGE: f32 = 200.0;

/// GPU-visible cascade data (set 1 binding 0 of the chunk pipeline).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ShadowData {
    /// Camera-relative world -> cascade clip.
    pub matrices: [Mat4; 3],
    /// Cascade far distances (x, y, z); w unused.
    pub splits: Vec4,
    /// x: shadow strength (0 = off / night); yzw: world units per shadow
    /// texel for each cascade (normal-offset scale).
    pub params: Vec4,
}

pub struct ShadowPass {
    render_pass: vk::RenderPass,
    pipeline: vk::Pipeline,
    pub pipeline_layout: vk::PipelineLayout,
    /// Set layout the chunk pipeline includes as set 1.
    pub descriptor_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    /// One per frame in flight (the UBO is rewritten per frame). Bound by the
    /// deferred lighting pass (set 2) for sun-shadow cascade sampling.
    pub descriptor_sets: Vec<vk::DescriptorSet>,
    sampler: vk::Sampler,
    image: vk::Image,
    allocation: Option<Allocation>,
    /// Layer views for the framebuffers plus one array view for sampling.
    layer_views: Vec<vk::ImageView>,
    array_view: vk::ImageView,
    framebuffers: Vec<vk::Framebuffer>,
    uniforms: Vec<(vk::Buffer, Allocation)>,
    /// The matrices computed by the latest `update`, for recording.
    cascades: [Mat4; 3],
    strength: f32,
    /// The map has been cleared at least once (its layout is valid), so
    /// inactive frames can skip the cascade passes entirely.
    primed: bool,
}

impl ShadowPass {
    pub unsafe fn new(
        ctx: &VulkanContext,
        allocator: &mut Allocator,
        frames_in_flight: usize,
    ) -> Result<Self> {
        unsafe {
            let device = &ctx.device;
            let render_pass = create_pass(device)?;

            // The cascade depth array.
            let image = device.create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(vk::Format::D32_SFLOAT)
                    .extent(vk::Extent3D { width: MAP_SIZE, height: MAP_SIZE, depth: 1 })
                    .mip_levels(1)
                    .array_layers(3)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(
                        vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT
                            | vk::ImageUsageFlags::SAMPLED,
                    )
                    .initial_layout(vk::ImageLayout::UNDEFINED),
                None,
            )?;
            let requirements = device.get_image_memory_requirements(image);
            let allocation = allocator.allocate(&AllocationCreateDesc {
                name: "shadow cascades",
                requirements,
                location: MemoryLocation::GpuOnly,
                linear: false,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })?;
            device.bind_image_memory(image, allocation.memory(), allocation.offset())?;

            let mut layer_views = Vec::new();
            let mut framebuffers = Vec::new();
            for layer in 0..3 {
                let view = device.create_image_view(
                    &vk::ImageViewCreateInfo::default()
                        .image(image)
                        .view_type(vk::ImageViewType::TYPE_2D)
                        .format(vk::Format::D32_SFLOAT)
                        .subresource_range(
                            vk::ImageSubresourceRange::default()
                                .aspect_mask(vk::ImageAspectFlags::DEPTH)
                                .level_count(1)
                                .base_array_layer(layer)
                                .layer_count(1),
                        ),
                    None,
                )?;
                layer_views.push(view);
                framebuffers.push(device.create_framebuffer(
                    &vk::FramebufferCreateInfo::default()
                        .render_pass(render_pass)
                        .attachments(std::slice::from_ref(&view))
                        .width(MAP_SIZE)
                        .height(MAP_SIZE)
                        .layers(1),
                    None,
                )?);
            }
            let array_view = device.create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D_ARRAY)
                    .format(vk::Format::D32_SFLOAT)
                    .subresource_range(
                        vk::ImageSubresourceRange::default()
                            .aspect_mask(vk::ImageAspectFlags::DEPTH)
                            .level_count(1)
                            .layer_count(3),
                    ),
                None,
            )?;

            // PCF comparison sampler: bilinear-of-comparisons.
            let sampler = device.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::LINEAR)
                    .min_filter(vk::Filter::LINEAR)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .compare_enable(true)
                    .compare_op(vk::CompareOp::LESS_OR_EQUAL),
                None,
            )?;

            let bindings = [
                vk::DescriptorSetLayoutBinding::default()
                    .binding(0)
                    .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
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
            ];
            let descriptor_layout = device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )?;
            let n = frames_in_flight as u32;
            let pool_sizes = [
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::UNIFORM_BUFFER)
                    .descriptor_count(n),
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(n),
                vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::SAMPLER)
                    .descriptor_count(n),
            ];
            let descriptor_pool = device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(n)
                    .pool_sizes(&pool_sizes),
                None,
            )?;

            let mut uniforms = Vec::new();
            let mut descriptor_sets = Vec::new();
            for i in 0..frames_in_flight {
                let buffer = device.create_buffer(
                    &vk::BufferCreateInfo::default()
                        .size(size_of::<ShadowData>() as u64)
                        .usage(vk::BufferUsageFlags::UNIFORM_BUFFER),
                    None,
                )?;
                let requirements = device.get_buffer_memory_requirements(buffer);
                let alloc = allocator.allocate(&AllocationCreateDesc {
                    name: &format!("shadow ubo {i}"),
                    requirements,
                    location: MemoryLocation::CpuToGpu,
                    linear: true,
                    allocation_scheme: AllocationScheme::GpuAllocatorManaged,
                })?;
                device.bind_buffer_memory(buffer, alloc.memory(), alloc.offset())?;

                let set = device.allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(descriptor_pool)
                        .set_layouts(std::slice::from_ref(&descriptor_layout)),
                )?[0];
                let buffer_info = [vk::DescriptorBufferInfo::default()
                    .buffer(buffer)
                    .range(size_of::<ShadowData>() as u64)];
                let image_info = [vk::DescriptorImageInfo::default()
                    .image_view(array_view)
                    .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
                let sampler_info = [vk::DescriptorImageInfo::default().sampler(sampler)];
                device.update_descriptor_sets(
                    &[
                        vk::WriteDescriptorSet::default()
                            .dst_set(set)
                            .dst_binding(0)
                            .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                            .buffer_info(&buffer_info),
                        vk::WriteDescriptorSet::default()
                            .dst_set(set)
                            .dst_binding(1)
                            .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                            .image_info(&image_info),
                        vk::WriteDescriptorSet::default()
                            .dst_set(set)
                            .dst_binding(2)
                            .descriptor_type(vk::DescriptorType::SAMPLER)
                            .image_info(&sampler_info),
                    ],
                    &[],
                );
                uniforms.push((buffer, alloc));
                descriptor_sets.push(set);
            }

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
                render_pass,
                pipeline,
                pipeline_layout,
                descriptor_layout,
                descriptor_pool,
                descriptor_sets,
                sampler,
                image,
                allocation: Some(allocation),
                layer_views,
                array_view,
                framebuffers,
                uniforms,
                cascades: [Mat4::IDENTITY; 3],
                strength: 0.0,
                primed: false,
            })
        }
    }

    /// Computes the cascades for this frame and writes the slot's UBO.
    /// `sun` is the daylight-scaled direction from FrameCamera (length =
    /// daylight); `enabled` is the settings toggle.
    pub fn update(&mut self, slot: usize, sun: Vec4, camera_pos: DVec3, enabled: bool) {
        let sun_dir = Vec3::new(sun.x, sun.y, sun.z);
        let daylight = sun_dir.length();
        // Fade through twilight: long, unstable shadows aren't worth it.
        let elevation = if daylight > 0.001 { sun_dir.y / daylight } else { 0.0 };
        self.strength = if enabled && daylight > 0.001 {
            smoothstep(0.06, 0.22, elevation) * smoothstep(0.0, 0.3, daylight)
        } else {
            0.0
        };

        let mut texels = [0.0f32; 3];
        if self.strength > 0.0 {
            let dir = sun_dir / daylight;
            // A stable light basis: up picked to never align with the sun.
            let up = if dir.y.abs() > 0.95 { Vec3::Z } else { Vec3::Y };
            let right = dir.cross(up).normalize();
            let lup = right.cross(dir);
            // Rows of the rotation: world -> light space.
            let rot = Mat4::from_cols(
                Vec4::new(right.x, lup.x, -dir.x, 0.0),
                Vec4::new(right.y, lup.y, -dir.y, 0.0),
                Vec4::new(right.z, lup.z, -dir.z, 0.0),
                Vec4::W,
            );
            for (i, radius) in RADII.iter().enumerate() {
                let texel = 2.0 * radius / MAP_SIZE as f32;
                texels[i] = texel;
                // Snap the cascade to its texel grid in light space using
                // the absolute camera position, so camera motion slides
                // the world through a fixed grid instead of shimmering.
                let cam_ls = (
                    (right.as_dvec3().dot(camera_pos)) as f32,
                    (lup.as_dvec3().dot(camera_pos)) as f32,
                );
                let snap = |v: f32| (v / texel).floor() * texel;
                let center = Vec3::new(snap(cam_ls.0) - cam_ls.0, snap(cam_ls.1) - cam_ls.1, 0.0);
                let proj = Mat4::orthographic_rh(
                    center.x - radius,
                    center.x + radius,
                    center.y - radius,
                    center.y + radius,
                    -DEPTH_RANGE,
                    DEPTH_RANGE,
                );
                self.cascades[i] = proj * rot;
            }
        }

        let data = ShadowData {
            matrices: self.cascades,
            splits: Vec4::new(RADII[0], RADII[1], RADII[2], 0.0),
            params: Vec4::new(self.strength, texels[0], texels[1], texels[2]),
        };
        if let Some(mapped) = self.uniforms[slot].1.mapped_slice_mut() {
            mapped[..size_of::<ShadowData>()].copy_from_slice(as_bytes(std::slice::from_ref(&data)));
        }
    }

    /// Whether the cascade passes should draw anything this frame.
    pub fn active(&self) -> bool {
        self.strength > 0.0
    }

    /// Whether the passes must run at all this frame: when active, or
    /// once at startup so the sampled image leaves UNDEFINED layout.
    pub fn needs_pass(&mut self) -> bool {
        let needs = self.strength > 0.0 || !self.primed;
        self.primed = true;
        needs
    }

    /// The cascade matrix for recording chunk draws (camera-relative).
    pub fn cascade(&self, index: usize) -> Mat4 {
        self.cascades[index]
    }

    /// Begins cascade pass `index`; the caller records chunk draws with
    /// `pipeline_layout` and ends the pass. Always run all three even when
    /// inactive (the clear keeps the map's layout and contents valid).
    pub unsafe fn begin(&self, device: &ash::Device, cmd: vk::CommandBuffer, index: usize) {
        unsafe {
            let extent = vk::Extent2D { width: MAP_SIZE, height: MAP_SIZE };
            device.cmd_set_viewport(
                cmd,
                0,
                &[vk::Viewport::default()
                    .width(MAP_SIZE as f32)
                    .height(MAP_SIZE as f32)
                    .max_depth(1.0)],
            );
            device.cmd_set_scissor(cmd, 0, &[extent.into()]);
            let clear = [vk::ClearValue {
                depth_stencil: vk::ClearDepthStencilValue { depth: 1.0, stencil: 0 },
            }];
            device.cmd_begin_render_pass(
                cmd,
                &vk::RenderPassBeginInfo::default()
                    .render_pass(self.render_pass)
                    .framebuffer(self.framebuffers[index])
                    .render_area(extent.into())
                    .clear_values(&clear),
                vk::SubpassContents::INLINE,
            );
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
        }
    }

    pub unsafe fn destroy(&mut self, device: &ash::Device, allocator: &mut Allocator) {
        unsafe {
            for (buffer, alloc) in self.uniforms.drain(..) {
                device.destroy_buffer(buffer, None);
                let _ = allocator.free(alloc);
            }
            for framebuffer in self.framebuffers.drain(..) {
                device.destroy_framebuffer(framebuffer, None);
            }
            device.destroy_image_view(self.array_view, None);
            for view in self.layer_views.drain(..) {
                device.destroy_image_view(view, None);
            }
            device.destroy_image(self.image, None);
            if let Some(allocation) = self.allocation.take() {
                let _ = allocator.free(allocation);
            }
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            device.destroy_sampler(self.sampler, None);
            device.destroy_descriptor_pool(self.descriptor_pool, None);
            device.destroy_descriptor_set_layout(self.descriptor_layout, None);
            device.destroy_render_pass(self.render_pass, None);
        }
    }
}

fn smoothstep(lo: f32, hi: f32, x: f32) -> f32 {
    let t = ((x - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Depth-only pass; ends SHADER_READ_ONLY for the world pass. Entry waits
/// for the previous frame's fragment reads of the map (WAR, execution
/// only); exit publishes the depth writes to fragment samplers.
unsafe fn create_pass(device: &ash::Device) -> Result<vk::RenderPass> {
    unsafe {
        let attachment = vk::AttachmentDescription::default()
            .format(vk::Format::D32_SFLOAT)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
        let depth_ref = vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
        let subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .depth_stencil_attachment(&depth_ref);
        let dependencies = [
            vk::SubpassDependency::default()
                .src_subpass(vk::SUBPASS_EXTERNAL)
                .dst_subpass(0)
                .src_stage_mask(
                    vk::PipelineStageFlags::FRAGMENT_SHADER
                        | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS
                        | vk::PipelineStageFlags::LATE_FRAGMENT_TESTS,
                )
                .src_access_mask(vk::AccessFlags::empty())
                .dst_stage_mask(
                    vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS
                        | vk::PipelineStageFlags::LATE_FRAGMENT_TESTS,
                )
                .dst_access_mask(vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE),
            vk::SubpassDependency::default()
                .src_subpass(0)
                .dst_subpass(vk::SUBPASS_EXTERNAL)
                .src_stage_mask(vk::PipelineStageFlags::LATE_FRAGMENT_TESTS)
                .src_access_mask(vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags::FRAGMENT_SHADER)
                .dst_access_mask(vk::AccessFlags::SHADER_READ),
        ];
        Ok(device.create_render_pass(
            &vk::RenderPassCreateInfo::default()
                .attachments(std::slice::from_ref(&attachment))
                .subpasses(std::slice::from_ref(&subpass))
                .dependencies(&dependencies),
            None,
        )?)
    }
}

unsafe fn create_pipeline(
    device: &ash::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> Result<vk::Pipeline> {
    unsafe {
        let code = ash::util::read_spv(&mut std::io::Cursor::new(SHADOW_SPV))?;
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
            .stride(8)
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
        // BACK culling, same as the main pass: greedy meshing emits only
        // the visible shell (no interior faces), so front-face culling
        // would leave the sun seeing almost nothing. Acne is handled by
        // the slope bias here plus the texel-scaled normal offset at
        // sampling time.
        let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::BACK)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .depth_bias_enable(true)
            .depth_bias_constant_factor(4.0)
            .depth_bias_slope_factor(4.0)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(true)
            .depth_write_enable(true)
            .depth_compare_op(vk::CompareOp::LESS);
        let blend = vk::PipelineColorBlendStateCreateInfo::default();
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

    /// A camera-relative point near the camera must land inside every
    /// cascade's NDC box, and a point along +X by R/2 must move in NDC.
    #[test]
    fn cascade_matrices_contain_the_camera() {
        // Mirror update()'s math without a GPU.
        let sun_dir = Vec3::new(0.78, 0.57, 0.24); // daylight-scaled
        let daylight = sun_dir.length();
        let dir = sun_dir / daylight;
        let up = if dir.y.abs() > 0.95 { Vec3::Z } else { Vec3::Y };
        let right = dir.cross(up).normalize();
        let lup = right.cross(dir);
        let rot = Mat4::from_cols(
            Vec4::new(right.x, lup.x, -dir.x, 0.0),
            Vec4::new(right.y, lup.y, -dir.y, 0.0),
            Vec4::new(right.z, lup.z, -dir.z, 0.0),
            Vec4::W,
        );
        let camera_pos = DVec3::new(-650.0, 8.0, 480.0);
        for radius in RADII {
            let texel = 2.0 * radius / MAP_SIZE as f32;
            let cam_ls = (
                (right.as_dvec3().dot(camera_pos)) as f32,
                (lup.as_dvec3().dot(camera_pos)) as f32,
            );
            let snap = |v: f32| (v / texel).floor() * texel;
            let center = Vec3::new(snap(cam_ls.0) - cam_ls.0, snap(cam_ls.1) - cam_ls.1, 0.0);
            let proj = Mat4::orthographic_rh(
                center.x - radius,
                center.x + radius,
                center.y - radius,
                center.y + radius,
                -DEPTH_RANGE,
                DEPTH_RANGE,
            );
            let vp = proj * rot;
            let near_cam = vp * Vec4::new(1.0, 2.0, 3.0, 1.0);
            assert!(
                near_cam.x.abs() < 1.0 && near_cam.y.abs() < 1.0,
                "camera-adjacent point outside cascade r={radius}: {near_cam:?}"
            );
            assert!(
                (0.0..=1.0).contains(&near_cam.z),
                "depth outside [0,1] for r={radius}: {near_cam:?}"
            );
            let off = vp * Vec4::new(radius * 0.5, 0.0, 0.0, 1.0);
            assert!(
                (off.x - near_cam.x).abs() > 0.1,
                "ortho not scaling for r={radius}"
            );
        }
    }
}
