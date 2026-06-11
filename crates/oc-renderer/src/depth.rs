//! Depth buffer, recreated with the swapchain.

use anyhow::Result;
use ash::vk;
use gpu_allocator::MemoryLocation;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme, Allocator};

use crate::context::VulkanContext;

pub const DEPTH_FORMAT: vk::Format = vk::Format::D32_SFLOAT;

pub struct DepthBuffer {
    pub image: vk::Image,
    pub view: vk::ImageView,
    allocation: Option<Allocation>,
}

impl DepthBuffer {
    pub unsafe fn new(
        ctx: &VulkanContext,
        allocator: &mut Allocator,
        extent: vk::Extent2D,
    ) -> Result<Self> {
        unsafe {
            let image = ctx.device.create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(DEPTH_FORMAT)
                    .extent(vk::Extent3D {
                        width: extent.width,
                        height: extent.height,
                        depth: 1,
                    })
                    .mip_levels(1)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT)
                    .initial_layout(vk::ImageLayout::UNDEFINED),
                None,
            )?;
            let requirements = ctx.device.get_image_memory_requirements(image);
            let allocation = allocator.allocate(&AllocationCreateDesc {
                name: "depth buffer",
                requirements,
                location: MemoryLocation::GpuOnly,
                linear: false,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })?;
            ctx.device
                .bind_image_memory(image, allocation.memory(), allocation.offset())?;
            let view = ctx.device.create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(DEPTH_FORMAT)
                    .subresource_range(
                        vk::ImageSubresourceRange::default()
                            .aspect_mask(vk::ImageAspectFlags::DEPTH)
                            .level_count(1)
                            .layer_count(1),
                    ),
                None,
            )?;
            Ok(Self {
                image,
                view,
                allocation: Some(allocation),
            })
        }
    }

    pub unsafe fn destroy(&mut self, device: &ash::Device, allocator: &mut Allocator) {
        unsafe {
            device.destroy_image_view(self.view, None);
            device.destroy_image(self.image, None);
            if let Some(allocation) = self.allocation.take() {
                let _ = allocator.free(allocation);
            }
        }
    }
}
