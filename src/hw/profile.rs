use derive_more::Display;
use std::fmt::{self, Formatter};

use super::device_profile_resolver::{
    DeviceProfileResolver, DeviceRoutingProfile, LedInfoResponse, TextPath,
};
use super::model::{FoundDevice, ServiceInfo};
use crate::protocol;

const ALTERNATE_VENDOR_SERVICE_UUID: &str = "0000ae00-0000-1000-8000-00805f9b34fb";
const UNUSABLE_WRITE_WITHOUT_RESPONSE_LIMIT: usize = 20;

/// Concrete panel dimensions in pixels.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct PanelDimensions {
    width: u16,
    height: u16,
}

impl PanelDimensions {
    /// Creates panel dimensions when both values are non-zero.
    ///
    /// ```
    /// use idm::PanelDimensions;
    ///
    /// let dimensions =
    ///     PanelDimensions::new(64, 64).expect("64x64 should be valid dimensions");
    /// assert_eq!(64, dimensions.width());
    /// assert_eq!(64, dimensions.height());
    /// ```
    #[must_use]
    pub const fn new(width: u16, height: u16) -> Option<Self> {
        if width == 0 || height == 0 {
            return None;
        }

        Some(Self { width, height })
    }

    /// Returns panel width in pixels.
    ///
    /// ```
    /// use idm::PanelDimensions;
    ///
    /// let dimensions =
    ///     PanelDimensions::new(8, 32).expect("8x32 should be valid dimensions");
    /// assert_eq!(8, dimensions.width());
    /// ```
    #[must_use]
    pub const fn width(self) -> u16 {
        self.width
    }

    /// Returns panel height in pixels.
    ///
    /// ```
    /// use idm::PanelDimensions;
    ///
    /// let dimensions =
    ///     PanelDimensions::new(8, 32).expect("8x32 should be valid dimensions");
    /// assert_eq!(32, dimensions.height());
    /// ```
    #[must_use]
    pub const fn height(self) -> u16 {
        self.height
    }
}

impl fmt::Display for PanelDimensions {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}x{}", self.width, self.height)
    }
}

/// Logical iDotMatrix panel size.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Display)]
pub enum PanelSize {
    /// A `8x32` panel.
    #[display("8x32")]
    Size8x32,
    /// A `16x16` panel.
    #[display("16x16")]
    Size16x16,
    /// A `16x32` panel.
    #[display("16x32")]
    Size16x32,
    /// A `16x64` panel.
    #[display("16x64")]
    Size16x64,
    /// A `24x48` panel.
    #[display("24x48")]
    Size24x48,
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
    panel_dimensions: Option<PanelDimensions>,
    routing_profile_present: bool,
    led_type: Option<u8>,
    text_path: Option<TextPath>,
    joint_mode: Option<u8>,
    gif_header_profile: GifHeaderProfile,
    image_upload_mode: ImageUploadMode,
    write_without_response_fallback: usize,
}

impl DeviceProfile {
    /// Creates a concrete device profile.
    ///
    /// ```
    /// use idm::{DeviceProfile, GifHeaderProfile, ImageUploadMode, PanelDimensions};
    ///
    /// let profile = DeviceProfile::new(
    ///     PanelDimensions::new(64, 64),
    ///     GifHeaderProfile::Timed,
    ///     ImageUploadMode::RawRgb,
    ///     514,
    /// );
    /// assert_eq!(514, profile.write_without_response_fallback());
    /// ```
    #[must_use]
    pub fn new(
        panel_dimensions: Option<PanelDimensions>,
        gif_header_profile: GifHeaderProfile,
        image_upload_mode: ImageUploadMode,
        write_without_response_fallback: usize,
    ) -> Self {
        Self {
            panel_dimensions,
            routing_profile_present: false,
            led_type: None,
            text_path: None,
            joint_mode: None,
            gif_header_profile,
            image_upload_mode,
            write_without_response_fallback,
        }
    }

    pub(crate) fn with_routing_profile(
        mut self,
        routing_profile: Option<DeviceRoutingProfile>,
    ) -> Self {
        self.routing_profile_present = routing_profile.is_some();
        self.led_type = routing_profile.and_then(|profile| profile.led_type);
        self.text_path = routing_profile.and_then(|profile| profile.text_path);
        self.joint_mode = routing_profile.and_then(|profile| profile.joint_mode);
        self
    }

    #[must_use]
    pub(crate) fn routing_profile_present(&self) -> bool {
        self.routing_profile_present
    }

    /// Returns the resolved panel dimensions, when known.
    ///
    /// ```
    /// use idm::{DeviceProfile, GifHeaderProfile, ImageUploadMode, PanelDimensions};
    ///
    /// let profile = DeviceProfile::new(
    ///     PanelDimensions::new(32, 32),
    ///     GifHeaderProfile::Timed,
    ///     ImageUploadMode::PngFile,
    ///     512,
    /// );
    /// assert_eq!(PanelDimensions::new(32, 32), profile.panel_dimensions());
    /// ```
    #[must_use]
    pub fn panel_dimensions(&self) -> Option<PanelDimensions> {
        self.panel_dimensions
    }

    /// Returns a coarse logical panel size classification.
    ///
    /// ```
    /// use idm::{DeviceProfile, GifHeaderProfile, ImageUploadMode, PanelDimensions, PanelSize};
    ///
    /// let profile = DeviceProfile::new(
    ///     PanelDimensions::new(32, 32),
    ///     GifHeaderProfile::Timed,
    ///     ImageUploadMode::PngFile,
    ///     512,
    /// );
    /// assert_eq!(PanelSize::Size32x32, profile.panel_size());
    /// ```
    #[must_use]
    pub fn panel_size(&self) -> PanelSize {
        self.panel_dimensions
            .map_or(PanelSize::Unknown, PanelSize::from)
    }

    /// Returns the resolved LED type, when known.
    ///
    /// ```
    /// use idm::{DeviceProfile, GifHeaderProfile, ImageUploadMode};
    ///
    /// let profile =
    ///     DeviceProfile::new(None, GifHeaderProfile::Timed, ImageUploadMode::PngFile, 512);
    /// assert_eq!(None, profile.led_type());
    /// ```
    #[must_use]
    pub fn led_type(&self) -> Option<u8> {
        self.led_type
    }

    /// Returns the resolved text path, when known.
    ///
    /// ```
    /// use idm::{DeviceProfile, GifHeaderProfile, ImageUploadMode};
    ///
    /// let profile =
    ///     DeviceProfile::new(None, GifHeaderProfile::Timed, ImageUploadMode::PngFile, 512);
    /// assert_eq!(None, profile.text_path());
    /// ```
    #[must_use]
    pub fn text_path(&self) -> Option<TextPath> {
        self.text_path
    }

    /// Returns the resolved joint mode, when required by ambiguous shapes.
    ///
    /// ```
    /// use idm::{DeviceProfile, GifHeaderProfile, ImageUploadMode};
    ///
    /// let profile =
    ///     DeviceProfile::new(None, GifHeaderProfile::Timed, ImageUploadMode::PngFile, 512);
    /// assert_eq!(None, profile.joint_mode());
    /// ```
    #[must_use]
    pub fn joint_mode(&self) -> Option<u8> {
        self.joint_mode
    }

    /// Returns the resolved GIF header profile.
    ///
    /// ```
    /// use idm::{DeviceProfile, GifHeaderProfile, ImageUploadMode};
    ///
    /// let profile = DeviceProfile::new(
    ///     None,
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
    /// use idm::{DeviceProfile, GifHeaderProfile, ImageUploadMode};
    ///
    /// let profile = DeviceProfile::new(
    ///     None,
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
    /// use idm::{DeviceProfile, GifHeaderProfile, ImageUploadMode};
    ///
    /// let profile = DeviceProfile::new(
    ///     None,
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

impl From<PanelDimensions> for PanelSize {
    fn from(value: PanelDimensions) -> Self {
        match (value.width(), value.height()) {
            (8, 32) => Self::Size8x32,
            (16, 16) => Self::Size16x16,
            (16, 32) => Self::Size16x32,
            (16, 64) => Self::Size16x64,
            (24, 48) => Self::Size24x48,
            (32, 32) => Self::Size32x32,
            (64, 64) => Self::Size64x64,
            _ => Self::Unknown,
        }
    }
}

pub(crate) fn resolve_device_profile(
    device: &FoundDevice,
    services: &[ServiceInfo],
    write_without_response_limit: Option<usize>,
    routing_profile: Option<DeviceRoutingProfile>,
) -> DeviceProfile {
    let panel_dimensions = routing_profile
        .and_then(|profile| profile.panel_size)
        .and_then(panel_dimensions_from_tuple)
        .or_else(|| {
            device
                .model_profile()
                .and_then(|model_profile| model_profile.panel_size)
                .and_then(panel_dimensions_from_tuple)
        })
        .or_else(|| infer_panel_dimensions(device.local_name()));
    let has_alternate_vendor_service = services.iter().any(|service| {
        service
            .uuid()
            .eq_ignore_ascii_case(ALTERNATE_VENDOR_SERVICE_UUID)
    });

    let image_upload_mode =
        if panel_dimensions == PanelDimensions::new(64, 64) || has_alternate_vendor_service {
            ImageUploadMode::RawRgb
        } else {
            ImageUploadMode::PngFile
        };

    let write_without_response_fallback = match write_without_response_limit {
        Some(limit) if limit > UNUSABLE_WRITE_WITHOUT_RESPONSE_LIMIT => limit,
        _ => protocol::TRANSPORT_CHUNK_FALLBACK,
    };

    DeviceProfile::new(
        panel_dimensions,
        GifHeaderProfile::Timed,
        image_upload_mode,
        write_without_response_fallback,
    )
    .with_routing_profile(routing_profile)
}

pub(crate) fn resolve_device_routing_profile(
    device: &FoundDevice,
    led_info: Option<LedInfoResponse>,
    selected_led_type: Option<u8>,
) -> Option<DeviceRoutingProfile> {
    if let Some(identity) = device.scan_identity() {
        let resolved = match selected_led_type {
            Some(selected) => DeviceProfileResolver::resolve_with_selected_led_type(
                identity,
                led_info,
                Some(selected),
            ),
            None => DeviceProfileResolver::resolve(identity, led_info),
        };
        return Some(resolved);
    }

    DeviceProfileResolver::resolve_without_scan_identity(led_info, selected_led_type)
}

fn panel_dimensions_from_tuple(panel_size: (u16, u16)) -> Option<PanelDimensions> {
    PanelDimensions::new(panel_size.0, panel_size.1)
}

fn infer_panel_dimensions(local_name: Option<&str>) -> Option<PanelDimensions> {
    let Some(local_name) = local_name else {
        return None;
    };
    let lower = local_name.to_ascii_lowercase();

    if lower.contains("64") {
        return PanelDimensions::new(64, 64);
    }
    if lower.contains("32") {
        return PanelDimensions::new(32, 32);
    }
    if lower.contains("16") {
        return PanelDimensions::new(16, 16);
    }

    None
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

        assert_eq!(
            DeviceProfile::new(
                PanelDimensions::new(64, 64),
                GifHeaderProfile::Timed,
                ImageUploadMode::RawRgb,
                protocol::TRANSPORT_CHUNK_FALLBACK,
            ),
            resolved
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

        assert_eq!(
            DeviceProfile::new(
                None,
                GifHeaderProfile::Timed,
                ImageUploadMode::PngFile,
                protocol::TRANSPORT_CHUNK_FALLBACK,
            ),
            resolved
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
            DeviceProfile::new(
                PanelDimensions::new(64, 64),
                GifHeaderProfile::Timed,
                ImageUploadMode::RawRgb,
                protocol::TRANSPORT_CHUNK_FALLBACK,
            ),
            resolved
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

        assert_eq!(
            DeviceProfile::new(
                PanelDimensions::new(64, 64),
                GifHeaderProfile::Timed,
                ImageUploadMode::RawRgb,
                protocol::TRANSPORT_CHUNK_FALLBACK,
            ),
            resolved
        );
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
