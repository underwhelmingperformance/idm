use std::io::Cursor;

use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, Rgb, RgbImage};
use thiserror::Error;

use crate::hw::PanelDimensions;

use super::Rgb888Frame;

/// Errors returned when preparing an image for panel upload.
#[derive(Debug, Error)]
pub enum ImagePreparationError {
    /// The source bytes are not a supported image format.
    #[error("failed to detect image format from source bytes")]
    UnknownFormat(#[source] image::ImageError),
    /// The source image failed to decode.
    #[error("failed to decode source image")]
    Decode(#[source] image::ImageError),
    /// The transformed image failed RGB888 framebuffer validation.
    #[error(transparent)]
    Frame(#[from] crate::Rgb888FrameError),
}

/// Prepared static image ready for protocol upload.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct PreparedStillImage {
    source_format: image::ImageFormat,
    frame: Rgb888Frame,
}

impl PreparedStillImage {
    /// Returns decoded source format.
    #[must_use]
    pub fn source_format(&self) -> image::ImageFormat {
        self.source_format
    }

    /// Returns RGB888 framebuffer ready for upload.
    #[must_use]
    pub fn frame(&self) -> &Rgb888Frame {
        &self.frame
    }

    /// Consumes the prepared image and returns upload frame bytes.
    #[must_use]
    pub fn into_frame(self) -> Rgb888Frame {
        self.frame
    }
}

/// Media preprocessor for image command input normalisation.
pub struct ImagePreprocessor;

impl ImagePreprocessor {
    /// Decodes, orients, resizes, and pads source bytes to panel geometry.
    ///
    /// The output framebuffer is always RGB888 with dimensions exactly matching
    /// `panel_dimensions`.
    ///
    /// # Errors
    ///
    /// Returns an error when format detection, decode, or framebuffer
    /// validation fails.
    pub fn prepare_still(
        source_bytes: &[u8],
        panel_dimensions: PanelDimensions,
    ) -> Result<PreparedStillImage, ImagePreparationError> {
        let source_format =
            image::guess_format(source_bytes).map_err(ImagePreparationError::UnknownFormat)?;
        let decoded = image::load_from_memory_with_format(source_bytes, source_format)
            .map_err(ImagePreparationError::Decode)?;
        let oriented = apply_exif_orientation(decoded, source_bytes);
        let padded = resize_and_pad(oriented, panel_dimensions);
        let frame = Rgb888Frame::try_from((panel_dimensions, padded.into_raw()))?;
        Ok(PreparedStillImage {
            source_format,
            frame,
        })
    }
}

fn apply_exif_orientation(image: DynamicImage, source_bytes: &[u8]) -> DynamicImage {
    let orientation = exif_orientation(source_bytes);
    match orientation {
        Some(2) => image.fliph(),
        Some(3) => image.rotate180(),
        Some(4) => image.flipv(),
        Some(5) => image.fliph().rotate90(),
        Some(6) => image.rotate90(),
        Some(7) => image.fliph().rotate270(),
        Some(8) => image.rotate270(),
        _ => image,
    }
}

fn exif_orientation(source_bytes: &[u8]) -> Option<u32> {
    let mut cursor = Cursor::new(source_bytes);
    let exif = exif::Reader::new().read_from_container(&mut cursor).ok()?;
    exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY)?
        .value
        .get_uint(0)
}

fn resize_and_pad(image: DynamicImage, panel_dimensions: PanelDimensions) -> RgbImage {
    let panel_width = u32::from(panel_dimensions.width());
    let panel_height = u32::from(panel_dimensions.height());
    let (source_width, source_height) = image.dimensions();

    let width_scaled_height =
        u64::from(source_height) * u64::from(panel_width) / u64::from(source_width);
    let (target_width, target_height) = if width_scaled_height <= u64::from(panel_height) {
        let safe_height = u32::try_from(width_scaled_height)
            .unwrap_or(panel_height)
            .max(1);
        (panel_width, safe_height)
    } else {
        let height_scaled_width =
            u64::from(source_width) * u64::from(panel_height) / u64::from(source_height);
        let safe_width = u32::try_from(height_scaled_width)
            .unwrap_or(panel_width)
            .max(1);
        (safe_width, panel_height)
    };

    let resized = image
        .resize_exact(target_width, target_height, FilterType::Lanczos3)
        .to_rgb8();
    let mut canvas = RgbImage::from_pixel(panel_width, panel_height, Rgb([0x00, 0x00, 0x00]));

    let offset_x = i64::from((panel_width - target_width) / 2);
    let offset_y = i64::from((panel_height - target_height) / 2);
    image::imageops::replace(&mut canvas, &resized, offset_x, offset_y);
    canvas
}

#[cfg(test)]
mod tests {
    use image::ImageEncoder;
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn prepare_still_outputs_panel_sized_rgb_frame() -> Result<(), Box<dyn std::error::Error>> {
        let mut png_bytes = Vec::new();
        let source = image::RgbaImage::from_pixel(2, 1, image::Rgba([0xAA, 0xBB, 0xCC, 0xFF]));
        image::codecs::png::PngEncoder::new(&mut png_bytes).write_image(
            source.as_raw(),
            2,
            1,
            image::ExtendedColorType::Rgba8,
        )?;

        let panel = PanelDimensions::new(4, 4).expect("4x4 should be valid");
        let prepared = ImagePreprocessor::prepare_still(&png_bytes, panel)?;

        assert_eq!(image::ImageFormat::Png, prepared.source_format());
        assert_eq!(panel, prepared.frame().dimensions());
        assert_eq!(48, prepared.frame().payload().len());
        Ok(())
    }
}
