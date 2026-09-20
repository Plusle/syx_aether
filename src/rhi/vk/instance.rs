use crate::rhi::error::RhiError;

use ash::{ext::debug_utils, vk, Entry};
use std::ffi::{c_void, CStr, CString};
use tracing::{debug, error, info, warn};

const VALIDATION_LAYER: &CStr = c"VK_LAYER_KHRONOS_validation";

pub struct Instance {
    pub instance: ash::Instance,
    pub debug_utils: Option<debug_utils::Instance>,
    pub debug_messenger: Option<vk::DebugUtilsMessengerEXT>,
    /// Dropped last (declaration order): instance must be destroyed before
    /// the entry's dynamic library is unloaded.
    _entry: Entry,
}

impl From<ash::LoadingError> for RhiError {
    fn from(error: ash::LoadingError) -> Self {
        RhiError::Shader(format!("Vulkan library loading error: {}", error))
    }
}

impl From<vk::Result> for RhiError {
    fn from(error: vk::Result) -> Self {
        RhiError::BackendError(format!("Vulkan error: {}", error))
    }
}

impl Instance {
    pub fn new(app_name: &str, enable_validation: bool) -> Result<Self, RhiError> {
        let entry = unsafe { Entry::load()? };

        let app_name = CString::new(app_name).unwrap_or_else(|_| CString::new("aether").unwrap());
        let engine_name = c"AetherEngine";

        let app_info = vk::ApplicationInfo::default()
            .application_name(&app_name)
            .application_version(vk::make_api_version(0, 0, 1, 0))
            .engine_name(engine_name)
            .engine_version(vk::make_api_version(0, 0, 1, 0))
            .api_version(vk::API_VERSION_1_3);

        let mut extensions = vec![];

        if enable_validation {
            extensions.push(vk::EXT_DEBUG_UTILS_NAME.as_ptr());
            extensions.push(vk::EXT_VALIDATION_FEATURES_NAME.as_ptr());
            // NOTE: ash 0.38 exposes the raw extension constants and structs for
            // VK_EXT_layer_settings, but does not provide a dedicated
            // `ash::extensions::ext::LayerSettings` loader module. We therefore
            // omit explicit layer-settings configuration here and rely on the
            // validation layer's defaults (syncval is enabled by default in
            // recent Vulkan SDKs).
        }

        let layers: Vec<*const i8> = if enable_validation {
            vec![VALIDATION_LAYER.as_ptr()]
        } else {
            vec![]
        };

        // These must outlive `create_instance` because the create info stores
        // pointers into them.
        let mut enabled_features: Vec<vk::ValidationFeatureEnableEXT> = Vec::new();
        let mut validation_features = vk::ValidationFeaturesEXT::default();

        let instance = {
            let create_info = if enable_validation {
                enabled_features.push(vk::ValidationFeatureEnableEXT::SYNCHRONIZATION_VALIDATION);
                validation_features =
                    validation_features.enabled_validation_features(&enabled_features);

                vk::InstanceCreateInfo::default()
                    .push_next(&mut validation_features)
                    .application_info(&app_info)
                    .enabled_extension_names(&extensions)
                    .enabled_layer_names(&layers)
            } else {
                vk::InstanceCreateInfo::default()
                    .application_info(&app_info)
                    .enabled_extension_names(&extensions)
                    .enabled_layer_names(&layers)
            };

            unsafe { entry.create_instance(&create_info, None)? }
        };
        info!(
            "Vulkan instance created (validation = {})",
            enable_validation
        );

        let (debug_utils, debug_messenger) = if enable_validation {
            let utils = debug_utils::Instance::new(&entry, &instance);
            let messenger_info = vk::DebugUtilsMessengerCreateInfoEXT::default()
                .message_severity(
                    vk::DebugUtilsMessageSeverityFlagsEXT::VERBOSE
                        | vk::DebugUtilsMessageSeverityFlagsEXT::WARNING
                        | vk::DebugUtilsMessageSeverityFlagsEXT::ERROR,
                )
                .message_type(
                    vk::DebugUtilsMessageTypeFlagsEXT::GENERAL
                        | vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                        | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
                )
                .pfn_user_callback(Some(debug_callback));

            let messenger = unsafe { utils.create_debug_utils_messenger(&messenger_info, None)? };
            debug!("Debug messenger created");
            (Some(utils), Some(messenger))
        } else {
            (None, None)
        };

        Ok(Self {
            instance,
            debug_utils,
            debug_messenger,
            _entry: entry,
        })
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            if let (Some(utils), Some(messenger)) =
                (self.debug_utils.as_ref(), self.debug_messenger)
            {
                utils.destroy_debug_utils_messenger(messenger, None);
                debug!("Debug messenger destroyed");
            }
            self.instance.destroy_instance(None);
            info!("Vulkan instance destroyed");
        }
    }
}

unsafe extern "system" fn debug_callback(
    message_severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    message_type: vk::DebugUtilsMessageTypeFlagsEXT,
    p_callback_data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>,
    _user_data: *mut c_void,
) -> vk::Bool32 {
    let data = &*p_callback_data;
    let message = if data.p_message.is_null() {
        "<null message>"
    } else {
        CStr::from_ptr(data.p_message)
            .to_str()
            .unwrap_or("<invalid utf8>")
    };

    // Ignore performance warnings; they are noisy and not errors for a bootstrap.
    if message_type.contains(vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE) {
        return vk::FALSE;
    }

    let labels = if data.queue_label_count > 0 && !data.p_queue_labels.is_null() {
        std::slice::from_raw_parts(data.p_queue_labels, data.queue_label_count as usize)
    } else {
        &[]
    };

    let objects = if data.object_count > 0 && !data.p_objects.is_null() {
        std::slice::from_raw_parts(data.p_objects, data.object_count as usize)
    } else {
        &[]
    };

    let label_names: Vec<&str> = labels
        .iter()
        .map(|l| {
            if l.p_label_name.is_null() {
                ""
            } else {
                CStr::from_ptr(l.p_label_name).to_str().unwrap_or("")
            }
        })
        .collect();

    let object_handles: Vec<u64> = objects.iter().map(|o| o.object_handle).collect();

    if message_severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR) {
        error!(
            target: "vulkan_validation",
            "[{:?}] {} | labels: {:?} | objects: {:?}",
            message_type,
            message,
            label_names,
            object_handles,
        );
    } else if message_severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::WARNING) {
        warn!(
            target: "vulkan_validation",
            "[{:?}] {} | labels: {:?} | objects: {:?}",
            message_type,
            message,
            label_names,
            object_handles,
        );
    } else {
        info!(
            target: "vulkan_validation",
            "[{:?}] {} | labels: {:?} | objects: {:?}",
            message_type,
            message,
            label_names,
            object_handles,
        );
    }

    vk::FALSE
}
