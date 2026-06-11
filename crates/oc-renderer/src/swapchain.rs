//! Swapchain creation and per-image views.

use anyhow::Result;
use ash::vk;
use tracing::debug;

use crate::context::VulkanContext;

pub struct Swapchain {
    pub handle: vk::SwapchainKHR,
    pub format: vk::Format,
    pub extent: vk::Extent2D,
    pub images: Vec<vk::Image>,
    pub image_views: Vec<vk::ImageView>,
    loader: ash::khr::swapchain::Device,
}

impl Swapchain {
    pub unsafe fn new(
        ctx: &VulkanContext,
        desired_extent: vk::Extent2D,
        old: Option<&Swapchain>,
    ) -> Result<Self> {
        unsafe {
            let caps = ctx.surface_loader.get_physical_device_surface_capabilities(
                ctx.physical_device,
                ctx.surface,
            )?;
            let formats = ctx
                .surface_loader
                .get_physical_device_surface_formats(ctx.physical_device, ctx.surface)?;

            let format = formats
                .iter()
                .find(|f| {
                    f.format == vk::Format::B8G8R8A8_SRGB
                        && f.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
                })
                .or_else(|| formats.first())
                .copied()
                .expect("surface reports no formats");

            // current_extent == u32::MAX means the surface lets us choose.
            let extent = if caps.current_extent.width != u32::MAX {
                caps.current_extent
            } else {
                vk::Extent2D {
                    width: desired_extent.width.clamp(
                        caps.min_image_extent.width,
                        caps.max_image_extent.width,
                    ),
                    height: desired_extent.height.clamp(
                        caps.min_image_extent.height,
                        caps.max_image_extent.height,
                    ),
                }
            };

            let mut image_count = caps.min_image_count + 1;
            if caps.max_image_count > 0 {
                image_count = image_count.min(caps.max_image_count);
            }

            let handle = ctx.swapchain_loader.create_swapchain(
                &vk::SwapchainCreateInfoKHR::default()
                    .surface(ctx.surface)
                    .min_image_count(image_count)
                    .image_format(format.format)
                    .image_color_space(format.color_space)
                    .image_extent(extent)
                    .image_array_layers(1)
                    .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
                    .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
                    .pre_transform(caps.current_transform)
                    .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
                    // FIFO (vsync) is the only mode Vulkan guarantees, and the
                    // right default for a game targeting a 60 fps cap.
                    .present_mode(vk::PresentModeKHR::FIFO)
                    .clipped(true)
                    .old_swapchain(old.map_or(vk::SwapchainKHR::null(), |s| s.handle)),
                None,
            )?;

            let images = ctx.swapchain_loader.get_swapchain_images(handle)?;
            let image_views = images
                .iter()
                .map(|&image| {
                    let view = ctx.device.create_image_view(
                        &vk::ImageViewCreateInfo::default()
                            .image(image)
                            .view_type(vk::ImageViewType::TYPE_2D)
                            .format(format.format)
                            .subresource_range(
                                vk::ImageSubresourceRange::default()
                                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                                    .level_count(1)
                                    .layer_count(1),
                            ),
                        None,
                    )?;
                    Ok(view)
                })
                .collect::<Result<Vec<_>>>()?;

            debug!(?extent, format = ?format.format, count = images.len(), "swapchain created");

            Ok(Self {
                handle,
                format: format.format,
                extent,
                images,
                image_views,
                loader: ctx.swapchain_loader.clone(),
            })
        }
    }

    /// Acquires the next image, returning its index.
    pub unsafe fn acquire(&self, signal: vk::Semaphore) -> Result<u32, vk::Result> {
        unsafe {
            let (index, _suboptimal) =
                self.loader
                    .acquire_next_image(self.handle, u64::MAX, signal, vk::Fence::null())?;
            Ok(index)
        }
    }

    /// Presents `image_index`; returns true if the swapchain is suboptimal.
    pub unsafe fn present(
        &self,
        queue: vk::Queue,
        image_index: u32,
        wait: vk::Semaphore,
    ) -> Result<bool, vk::Result> {
        unsafe {
            self.loader.queue_present(
                queue,
                &vk::PresentInfoKHR::default()
                    .wait_semaphores(std::slice::from_ref(&wait))
                    .swapchains(std::slice::from_ref(&self.handle))
                    .image_indices(std::slice::from_ref(&image_index)),
            )
        }
    }

    pub unsafe fn destroy(&self, ctx: &VulkanContext) {
        unsafe {
            for &view in &self.image_views {
                ctx.device.destroy_image_view(view, None);
            }
            ctx.swapchain_loader.destroy_swapchain(self.handle, None);
        }
    }
}
