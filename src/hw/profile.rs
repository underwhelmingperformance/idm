use derive_more::Display;

use super::device_profile_resolver::{
    DeviceProfileResolver, DeviceRoutingProfile, LedInfoResponse,
};
use super::model::{FoundDevice, ServiceInfo};

const ALTERNATE_VENDOR_SERVICE_UUID: &str = "0000ae00-0000-1000-8000-00805f9b34fb";
const DEFAULT_WRITE_WITHOUT_RESPONSE_FALLBACK: usize = 512;
const SIZE_64_WRITE_WITHOUT_RESPONSE_FALLBACK: usize = 514;
const UNUSABLE_WRITE_WITHOUT_RESPONSE_LIMIT: usize = 20;

/// Logical iDotMatrix panel size.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Display)]
pub enum PanelSize {
    /// A `16x16` panel.
    #[display("16x16")]
    Size16x16,
    /// A `32x32` panel.
    #[display("32x32")]
    Size32x32,
    /// A `64x64` panel.
    #[display("64x64")]
    Size64x64,
    /// Unknown panel dimensions.
    #[display("unknown")]
    Unknown,
}

/// GIF header profile used when encoding bytes `13..15`.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Display)]
pub enum GifHeaderProfile {
    /// Timed profile (`time_hi time_lo type`), commonly ending in `... 05 00 0D`.
    #[display("timed")]
    Timed,
    /// No-time-signature profile (`00 00 0C`).
    #[display("no_time_signature")]
    NoTimeSignature,
}

/// Image upload mode to use for DIY/media handlers.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Display)]
pub enum ImageUploadMode {
    /// PNG file-byte upload.
    #[display("png_file")]
    PngFile,
    /// Raw RGB upload.
    #[display("raw_rgb")]
    RawRgb,
}

/// Resolved device behaviour profile.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct DeviceProfile {
    panel_size: PanelSize,
    gif_header_profile: GifHeaderProfile,
    image_upload_mode: ImageUploadMode,
    write_without_response_fallback: usize,
}

impl DeviceProfile {
    /// Creates a concrete device profile.
    ///
    /// ```
    /// use idm::{DeviceProfile, GifHeaderProfile, ImageUploadMode, PanelSize};
    ///
    /// let profile = DeviceProfile::new(
    ///     PanelSize::Size64x64,
    ///     GifHeaderProfile::Timed,
    ///     ImageUploadMode::RawRgb,
    ///     514,
    /// );
    /// assert_eq!(514, profile.write_without_response_fallback());
    /// ```
    #[must_use]
    pub fn new(
        panel_size: PanelSize,
        gif_header_profile: GifHeaderProfile,
        image_upload_mode: ImageUploadMode,
        write_without_response_fallback: usize,
    ) -> Self {
        Self {
            panel_size,
            gif_header_profile,
            image_upload_mode,
            write_without_response_fallback,
        }
    }

    /// Returns the resolved panel size.
    ///
    /// ```
    /// use idm::{DeviceProfile, GifHeaderProfile, ImageUploadMode, PanelSize};
    ///
    /// let profile = DeviceProfile::new(
    ///     PanelSize::Size32x32,
    ///     GifHeaderProfile::Timed,
    ///     ImageUploadMode::PngFile,
    ///     512,
    /// );
    /// assert_eq!(PanelSize::Size32x32, profile.panel_size());
    /// ```
    #[must_use]
    pub fn panel_size(&self) -> PanelSize {
        self.panel_size
    }

    /// Returns the resolved GIF header profile.
    ///
    /// ```
    /// use idm::{DeviceProfile, GifHeaderProfile, ImageUploadMode, PanelSize};
    ///
    /// let profile = DeviceProfile::new(
    ///     PanelSize::Unknown,
    ///     GifHeaderProfile::NoTimeSignature,
    ///     ImageUploadMode::PngFile,
    ///     512,
    /// );
    /// assert_eq!(GifHeaderProfile::NoTimeSignature, profile.gif_header_profile());
    /// ```
    #[must_use]
    pub fn gif_header_profile(&self) -> GifHeaderProfile {
        self.gif_header_profile
    }

    /// Returns the resolved image upload mode.
    ///
    /// ```
    /// use idm::{DeviceProfile, GifHeaderProfile, ImageUploadMode, PanelSize};
    ///
    /// let profile = DeviceProfile::new(
    ///     PanelSize::Unknown,
    ///     GifHeaderProfile::Timed,
    ///     ImageUploadMode::RawRgb,
    ///     514,
    /// );
    /// assert_eq!(ImageUploadMode::RawRgb, profile.image_upload_mode());
    /// ```
    #[must_use]
    pub fn image_upload_mode(&self) -> ImageUploadMode {
        self.image_upload_mode
    }

    /// Returns the write-without-response fallback chunk size.
    ///
    /// ```
    /// use idm::{DeviceProfile, GifHeaderProfile, ImageUploadMode, PanelSize};
    ///
    /// let profile = DeviceProfile::new(
    ///     PanelSize::Unknown,
    ///     GifHeaderProfile::Timed,
    ///     ImageUploadMode::PngFile,
    ///     512,
    /// );
    /// assert_eq!(512, profile.write_without_response_fallback());
    /// ```
    #[must_use]
    pub fn write_without_response_fallback(&self) -> usize {
        self.write_without_response_fallback
    }
}

pub(crate) fn resolve_device_profile(
    device: &FoundDevice,
    services: &[ServiceInfo],
    write_without_response_limit: Option<usize>,
    routing_profile: Option<DeviceRoutingProfile>,
) -> DeviceProfile {
    let panel_size = routing_profile
        .and_then(|profile| profile.panel_size)
        .and_then(panel_size_from_dimensions)
        .or_else(|| {
            device
                .model_profile()
                .and_then(|model_profile| model_profile.panel_size)
                .and_then(panel_size_from_dimensions)
        })
        .unwrap_or_else(|| infer_panel_size(device.local_name()));
    let has_alternate_vendor_service = services.iter().any(|service| {
        service
            .uuid()
            .eq_ignore_ascii_case(ALTERNATE_VENDOR_SERVICE_UUID)
    });

    let image_upload_mode =
        if matches!(panel_size, PanelSize::Size64x64) || has_alternate_vendor_service {
            ImageUploadMode::RawRgb
        } else {
            ImageUploadMode::PngFile
        };

    let write_without_response_fallback = match write_without_response_limit {
        Some(limit) if limit > UNUSABLE_WRITE_WITHOUT_RESPONSE_LIMIT => limit,
        _ if matches!(panel_size, PanelSize::Size64x64) => SIZE_64_WRITE_WITHOUT_RESPONSE_FALLBACK,
        _ => DEFAULT_WRITE_WITHOUT_RESPONSE_FALLBACK,
    };

    DeviceProfile::new(
        panel_size,
        GifHeaderProfile::Timed,
        image_upload_mode,
        write_without_response_fallback,
    )
}

pub(crate) fn resolve_device_routing_profile(
    device: &FoundDevice,
    led_info: Option<LedInfoResponse>,
    selected_led_type: Option<u8>,
) -> Option<DeviceRoutingProfile> {
    if let Some(identity) = device.scan_identity() {
        return Some(DeviceProfileResolver::resolve_with_selected_led_type(
            identity,
            led_info,
            selected_led_type,
        ));
    }

    DeviceProfileResolver::resolve_without_scan_identity(led_info, selected_led_type)
}

fn panel_size_from_dimensions(panel_size: (u16, u16)) -> Option<PanelSize> {
    match panel_size {
        (16, 16) => Some(PanelSize::Size16x16),
        (32, 32) => Some(PanelSize::Size32x32),
        (64, 64) => Some(PanelSize::Size64x64),
        _ => None,
    }
}

fn infer_panel_size(local_name: Option<&str>) -> PanelSize {
    let Some(local_name) = local_name else {
        return PanelSize::Unknown;
    };
    let lower = local_name.to_ascii_lowercase();

    if lower.contains("64") {
        return PanelSize::Size64x64;
    }
    if lower.contains("32") {
        return PanelSize::Size32x32;
    }
    if lower.contains("16") {
        return PanelSize::Size16x16;
    }

    PanelSize::Unknown
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::hw::scan_model::{ModelProfile, ScanIdentity};
    use crate::hw::{CharacteristicInfo, FoundDevice, ServiceInfo, TextPath};

    fn device(local_name: Option<&str>) -> FoundDevice {
        FoundDevice::new(
            "hci0".to_string(),
            "AA:BB:CC".to_string(),
            local_name.map(ToString::to_string),
            Some(-40),
        )
    }

    fn device_with_model(local_name: Option<&str>, panel_size: Option<(u16, u16)>) -> FoundDevice {
        let scan_identity = ScanIdentity {
            cid: 1,
            pid: 5,
            shape: 4,
            reverse: false,
            group_id: 1,
            device_id: 2,
            lamp_count: 64,
            lamp_num: 64,
        };
        let model_profile = ModelProfile {
            led_type: Some(4),
            panel_size,
            ambiguous_shape: None,
        };

        device(local_name).with_scan_model(scan_identity, model_profile)
    }

    fn primary_fa_service() -> ServiceInfo {
        ServiceInfo::new(
            "000000fa-0000-1000-8000-00805f9b34fb".to_string(),
            true,
            vec![
                CharacteristicInfo::new(
                    "0000fa02-0000-1000-8000-00805f9b34fb".to_string(),
                    vec!["write".to_string()],
                ),
                CharacteristicInfo::new(
                    "0000fa03-0000-1000-8000-00805f9b34fb".to_string(),
                    vec!["notify".to_string()],
                ),
            ],
        )
    }

    #[test]
    fn resolver_uses_64_profile_hints_from_name() {
        let resolved = resolve_device_profile(
            &device(Some("IDM-64X64")),
            &[primary_fa_service()],
            None,
            None,
        );

        assert_eq!(PanelSize::Size64x64, resolved.panel_size());
        assert_eq!(ImageUploadMode::RawRgb, resolved.image_upload_mode());
        assert_eq!(
            SIZE_64_WRITE_WITHOUT_RESPONSE_FALLBACK,
            resolved.write_without_response_fallback()
        );
    }

    #[test]
    fn resolver_falls_back_to_png_profile_for_unknown_models() {
        let resolved = resolve_device_profile(
            &device(Some("IDM-Clock")),
            &[primary_fa_service()],
            None,
            None,
        );

        assert_eq!(PanelSize::Unknown, resolved.panel_size());
        assert_eq!(ImageUploadMode::PngFile, resolved.image_upload_mode());
        assert_eq!(
            DEFAULT_WRITE_WITHOUT_RESPONSE_FALLBACK,
            resolved.write_without_response_fallback()
        );
    }

    #[test]
    fn resolver_ignores_unusable_reported_write_limit() {
        let resolved = resolve_device_profile(
            &device(Some("IDM-64")),
            &[primary_fa_service()],
            Some(UNUSABLE_WRITE_WITHOUT_RESPONSE_LIMIT),
            None,
        );

        assert_eq!(
            SIZE_64_WRITE_WITHOUT_RESPONSE_FALLBACK,
            resolved.write_without_response_fallback()
        );
    }

    #[test]
    fn resolver_prefers_scan_model_panel_over_local_name_heuristics() {
        let resolved = resolve_device_profile(
            &device_with_model(Some("IDM-Clock"), Some((64, 64))),
            &[primary_fa_service()],
            None,
            None,
        );

        assert_eq!(PanelSize::Size64x64, resolved.panel_size());
        assert_eq!(ImageUploadMode::RawRgb, resolved.image_upload_mode());
    }

    #[test]
    fn resolve_device_routing_profile_uses_scan_identity() {
        let device = device_with_model(Some("IDM-Clock"), Some((64, 64)));
        let resolved = resolve_device_routing_profile(&device, None, None);

        assert_eq!(
            Some(DeviceRoutingProfile {
                led_type: Some(4),
                panel_size: Some((64, 64)),
                text_path: Some(TextPath::Path6464),
                joint_mode: None,
            }),
            resolved
        );
    }

    #[test]
    fn resolve_device_routing_profile_falls_back_to_led_info_without_scan_identity() {
        let device = device(Some("IDM-Clock"));
        let led_info = Some(LedInfoResponse {
            mcu_major_version: 1,
            mcu_minor_version: 0,
            status: 0,
            screen_type: 4,
            password_enabled: false,
        });
        let resolved = resolve_device_routing_profile(&device, led_info, None);

        assert_eq!(
            Some(DeviceRoutingProfile {
                led_type: Some(4),
                panel_size: Some((64, 64)),
                text_path: Some(TextPath::Path6464),
                joint_mode: None,
            }),
            resolved
        );
    }
}
