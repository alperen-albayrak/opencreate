//! Vulkan instance, surface and device creation, including the MoltenVK
//! portability handling that macOS requires (ARCHITECTURE.md §4).

use std::ffi::CStr;

use anyhow::{Context as _, Result, bail};
use ash::vk;
use raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use tracing::info;

/// Core Vulkan objects shared by everything that renders.
pub struct VulkanContext {
    /// Owns the loaded Vulkan library; must stay alive or every function
    /// pointer in the structs below dangles.
    pub _entry: ash::Entry,
    pub instance: ash::Instance,
    pub surface_loader: ash::khr::surface::Instance,
    pub surface: vk::SurfaceKHR,
    pub physical_device: vk::PhysicalDevice,
    pub device: ash::Device,
    pub swapchain_loader: ash::khr::swapchain::Device,
    pub queue_family: u32,
    pub queue: vk::Queue,
}

impl VulkanContext {
    /// # Safety
    /// The window behind the raw handles must outlive the context.
    pub unsafe fn new(display: RawDisplayHandle, window: RawWindowHandle) -> Result<Self> {
        unsafe {
            let entry = load_entry()?;

            let app_info = vk::ApplicationInfo::default()
                .application_name(c"opencreate")
                .engine_name(c"opencreate")
                .api_version(vk::API_VERSION_1_2);

            let mut extensions: Vec<*const i8> =
                ash_window::enumerate_required_extensions(display)?.to_vec();
            let mut flags = vk::InstanceCreateFlags::empty();
            // MoltenVK is a non-conformant ("portability") implementation; the
            // loader hides it unless we opt in. Without these two lines Vulkan
            // simply reports no GPUs on macOS.
            let available = entry.enumerate_instance_extension_properties(None)?;
            let has_portability = available.iter().any(|ext| {
                ext.extension_name_as_c_str()
                    == Ok(ash::khr::portability_enumeration::NAME)
            });
            if has_portability {
                extensions.push(ash::khr::portability_enumeration::NAME.as_ptr());
                extensions.push(ash::khr::get_physical_device_properties2::NAME.as_ptr());
                flags |= vk::InstanceCreateFlags::ENUMERATE_PORTABILITY_KHR;
            }

            let instance = entry.create_instance(
                &vk::InstanceCreateInfo::default()
                    .application_info(&app_info)
                    .enabled_extension_names(&extensions)
                    .flags(flags),
                None,
            )?;

            let surface_loader = ash::khr::surface::Instance::new(&entry, &instance);
            let surface = ash_window::create_surface(&entry, &instance, display, window, None)?;

            let (physical_device, queue_family) =
                pick_device(&instance, &surface_loader, surface)?;

            let props = instance.get_physical_device_properties(physical_device);
            info!(
                device = %CStr::from_ptr(props.device_name.as_ptr()).to_string_lossy(),
                api = %format_args!(
                    "{}.{}.{}",
                    vk::api_version_major(props.api_version),
                    vk::api_version_minor(props.api_version),
                    vk::api_version_patch(props.api_version),
                ),
                "selected GPU"
            );

            let mut device_extensions = vec![ash::khr::swapchain::NAME.as_ptr()];
            // If the implementation is a portability subset (MoltenVK), the
            // spec requires enabling the extension that says so.
            let dev_exts = instance.enumerate_device_extension_properties(physical_device)?;
            if dev_exts.iter().any(|ext| {
                ext.extension_name_as_c_str() == Ok(ash::khr::portability_subset::NAME)
            }) {
                device_extensions.push(ash::khr::portability_subset::NAME.as_ptr());
            }

            let queue_info = vk::DeviceQueueCreateInfo::default()
                .queue_family_index(queue_family)
                .queue_priorities(&[1.0]);
            let device = instance.create_device(
                physical_device,
                &vk::DeviceCreateInfo::default()
                    .queue_create_infos(std::slice::from_ref(&queue_info))
                    .enabled_extension_names(&device_extensions),
                None,
            )?;
            let queue = device.get_device_queue(queue_family, 0);
            let swapchain_loader = ash::khr::swapchain::Device::new(&instance, &device);

            Ok(Self {
                _entry: entry,
                instance,
                surface_loader,
                surface,
                physical_device,
                device,
                swapchain_loader,
                queue_family,
                queue,
            })
        }
    }
}

impl Drop for VulkanContext {
    fn drop(&mut self) {
        unsafe {
            let _ = self.device.device_wait_idle();
            self.device.destroy_device(None);
            self.surface_loader.destroy_surface(self.surface, None);
            self.instance.destroy_instance(None);
        }
    }
}

/// Loads the Vulkan loader, falling back to well-known Homebrew paths on
/// macOS where `dlopen("libvulkan.dylib")` does not search /opt/homebrew/lib.
unsafe fn load_entry() -> Result<ash::Entry> {
    unsafe {
        match ash::Entry::load() {
            Ok(entry) => Ok(entry),
            Err(primary) if cfg!(target_os = "macos") => {
                for path in [
                    "/opt/homebrew/lib/libvulkan.dylib",
                    "/usr/local/lib/libvulkan.dylib",
                    "/opt/homebrew/lib/libMoltenVK.dylib",
                ] {
                    if let Ok(entry) = ash::Entry::load_from(path) {
                        info!(path, "loaded Vulkan via fallback path");
                        return Ok(entry);
                    }
                }
                Err(primary).context(
                    "no Vulkan loader found; install it with `brew install vulkan-loader molten-vk`",
                )
            }
            Err(e) => Err(e).context("no Vulkan loader found"),
        }
    }
}

/// Picks the first GPU with a queue family that does both graphics and
/// present, preferring discrete GPUs.
unsafe fn pick_device(
    instance: &ash::Instance,
    surface_loader: &ash::khr::surface::Instance,
    surface: vk::SurfaceKHR,
) -> Result<(vk::PhysicalDevice, u32)> {
    unsafe {
        let mut best: Option<(vk::PhysicalDevice, u32, bool)> = None;
        for device in instance.enumerate_physical_devices()? {
            let families = instance.get_physical_device_queue_family_properties(device);
            let family = families.iter().enumerate().find_map(|(index, family)| {
                let index = index as u32;
                let graphics = family.queue_flags.contains(vk::QueueFlags::GRAPHICS);
                let present = surface_loader
                    .get_physical_device_surface_support(device, index, surface)
                    .unwrap_or(false);
                (graphics && present).then_some(index)
            });
            let Some(family) = family else { continue };

            let discrete = instance.get_physical_device_properties(device).device_type
                == vk::PhysicalDeviceType::DISCRETE_GPU;
            if discrete {
                return Ok((device, family));
            }
            if best.is_none() {
                best = Some((device, family, discrete));
            }
        }
        match best {
            Some((device, family, _)) => Ok((device, family)),
            None => bail!("no GPU with graphics + present support found"),
        }
    }
}
