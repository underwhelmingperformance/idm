use std::io::Cursor;

use image::imageops::FilterType;
use image::{AnimationDecoder, Delay, DynamicImage, GenericImageView};
use thiserror::Error;

use crate::hw::PanelDimensions;

use super::{GifAnimation, Rgb888Frame};

/// Errors returned when preparing an image for panel upload.
#[derive(Debug, Error)]
pub enum ImagePreparationError {
    /// The source bytes are not a supported image format.
    #[error("failed to detect image format from source bytes")]
    UnknownFormat(#[source] image::ImageError),
    /// The source image failed to decode.
    #[error("failed to decode source image")]
    Decode(#[source] image::ImageError),
    /// GIF re-encoding failed after frame transformation.
    #[error("failed to encode transformed gif payload")]
    GifEncode { source: gif::EncodingError },
    /// The GIF stream does not contain any frames.
    #[error("gif payload contains no frames")]
    GifHasNoFrames,
    /// Transformed GIF payload failed GIF validation.
    #[error(transparent)]
    GifPayload(#[from] crate::GifAnimationError),
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

/// Prepared media payload routed to the correct upload endpoint.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum PreparedImageUpload {
    /// Static image transformed into one RGB888 frame.
    Still(PreparedStillImage),
    /// GIF transformed to panel geometry and re-encoded.
    Gif(GifAnimation),
}

impl PreparedImageUpload {
    /// Returns the detected source format.
    #[must_use]
    pub fn source_format(&self) -> image::ImageFormat {
        match self {
            Self::Still(still) => still.source_format(),
            Self::Gif(_gif) => image::ImageFormat::Gif,
        }
    }
}

/// Media preprocessor for image command input normalisation.
pub struct ImagePreprocessor;

impl ImagePreprocessor {
    /// Decodes, orients, resizes, and pads source bytes to panel geometry.
    ///
    /// Static images are transformed into one RGB888 frame. GIF files are
    /// transformed frame-by-frame and re-encoded as GIF before upload.
    ///
    /// # Errors
    ///
    /// Returns an error when format detection, decode, transformation, encode,
    /// or payload validation fails.
    pub fn prepare_for_upload(
        source_bytes: &[u8],
        panel_dimensions: PanelDimensions,
    ) -> Result<PreparedImageUpload, ImagePreparationError> {
        let source_format =
            image::guess_format(source_bytes).map_err(ImagePreparationError::UnknownFormat)?;
        match source_format {
            image::ImageFormat::Gif => {
                let gif = Self::prepare_gif(source_bytes, panel_dimensions)?;
                Ok(PreparedImageUpload::Gif(gif))
            }
            _other => {
                let still = Self::prepare_still(source_bytes, panel_dimensions, source_format)?;
                Ok(PreparedImageUpload::Still(still))
            }
        }
    }

    fn prepare_still(
        source_bytes: &[u8],
        panel_dimensions: PanelDimensions,
        source_format: image::ImageFormat,
    ) -> Result<PreparedStillImage, ImagePreparationError> {
        let decoded = image::load_from_memory_with_format(source_bytes, source_format)
            .map_err(ImagePreparationError::Decode)?;
        let oriented = apply_orientation(decoded, exif_orientation(source_bytes));
        let padded =
            DynamicImage::ImageRgba8(resize_and_pad_rgba(oriented, panel_dimensions)).to_rgb8();
        let frame = Rgb888Frame::try_from((panel_dimensions, padded.into_raw()))?;
        Ok(PreparedStillImage {
            source_format,
            frame,
        })
    }

    fn prepare_gif(
        source_bytes: &[u8],
        panel_dimensions: PanelDimensions,
    ) -> Result<GifAnimation, ImagePreparationError> {
        let decoder = image::codecs::gif::GifDecoder::new(Cursor::new(source_bytes))
            .map_err(ImagePreparationError::Decode)?;
        let frames = decoder
            .into_frames()
            .collect_frames()
            .map_err(ImagePreparationError::Decode)?;
        if frames.is_empty() {
            return Err(ImagePreparationError::GifHasNoFrames);
        }

        let orientation = exif_orientation(source_bytes);
        let panel_width = panel_dimensions.width();
        let panel_height = panel_dimensions.height();

        let mut transformed_payload = Vec::new();
        {
            let mut encoder =
                gif::Encoder::new(&mut transformed_payload, panel_width, panel_height, &[])
                    .map_err(|source| ImagePreparationError::GifEncode { source })?;
            encoder
                .set_repeat(gif::Repeat::Infinite)
                .map_err(|source| ImagePreparationError::GifEncode { source })?;

            for frame in frames {
                let delay = frame.delay();
                let dynamic = DynamicImage::ImageRgba8(frame.into_buffer());
                let oriented = apply_orientation(dynamic, orientation);
                let padded = resize_and_pad_rgba(oriented, panel_dimensions);
                let mut rgba_bytes = padded.into_raw();
                let mut encoded_frame =
                    gif::Frame::from_rgba_speed(panel_width, panel_height, &mut rgba_bytes, 10);
                encoded_frame.delay = delay_to_centiseconds(delay);
                encoder
                    .write_frame(&encoded_frame)
                    .map_err(|source| ImagePreparationError::GifEncode { source })?;
            }
        }

        Ok(GifAnimation::try_from(transformed_payload)?)
    }
}

fn apply_orientation(image: DynamicImage, orientation: Option<u32>) -> DynamicImage {
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

fn resize_and_pad_rgba(image: DynamicImage, panel_dimensions: PanelDimensions) -> image::RgbaImage {
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
        .to_rgba8();
    let mut canvas = image::RgbaImage::from_pixel(
        panel_width,
        panel_height,
        image::Rgba([0x00, 0x00, 0x00, 0xFF]),
    );

    let offset_x = i64::from((panel_width - target_width) / 2);
    let offset_y = i64::from((panel_height - target_height) / 2);
    image::imageops::replace(&mut canvas, &resized, offset_x, offset_y);
    canvas
}

fn delay_to_centiseconds(delay: Delay) -> u16 {
    let (numerator, denominator) = delay.numer_denom_ms();
    if denominator == 0 {
        return 0;
    }
    let millis = (u64::from(numerator) + (u64::from(denominator) / 2)) / u64::from(denominator);
    let centiseconds = millis.div_ceil(10);
    u16::try_from(centiseconds).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use image::ImageEncoder;
    use pretty_assertions::assert_eq;

    use super::*;

    const MINIMAL_GIF_1X1: [u8; 43] = [
        0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00,
        0x00, 0xFF, 0xFF, 0xFF, 0x21, 0xF9, 0x04, 0x01, 0x00, 0x00, 0x00, 0x00, 0x2C, 0x00, 0x00,
        0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02, 0x44, 0x01, 0x00, 0x3B,
    ];

    #[test]
    fn prepare_for_upload_outputs_panel_sized_rgb_frame() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut png_bytes = Vec::new();
        let source = image::RgbaImage::from_pixel(2, 1, image::Rgba([0xAA, 0xBB, 0xCC, 0xFF]));
        image::codecs::png::PngEncoder::new(&mut png_bytes).write_image(
            source.as_raw(),
            2,
            1,
            image::ExtendedColorType::Rgba8,
        )?;

        let panel = PanelDimensions::new(4, 4).expect("4x4 should be valid");
        let prepared = ImagePreprocessor::prepare_for_upload(&png_bytes, panel)?;

        match prepared {
            PreparedImageUpload::Still(still) => {
                assert_eq!(image::ImageFormat::Png, still.source_format());
                assert_eq!(panel, still.frame().dimensions());
                assert_eq!(48, still.frame().payload().len());
            }
            PreparedImageUpload::Gif(_gif) => panic!("png should produce still upload"),
        }
        Ok(())
    }

    #[test]
    fn prepare_for_upload_transforms_gif_to_panel_dimensions()
    -> Result<(), Box<dyn std::error::Error>> {
        let panel = PanelDimensions::new(4, 4).expect("4x4 should be valid");
        let prepared = ImagePreprocessor::prepare_for_upload(&MINIMAL_GIF_1X1, panel)?;

        match prepared {
            PreparedImageUpload::Still(_still) => panic!("gif should produce gif upload"),
            PreparedImageUpload::Gif(gif) => {
                assert_eq!(panel, gif.dimensions());
                assert!(gif.payload().len() >= MINIMAL_GIF_1X1.len());
            }
        }
        Ok(())
    }
}
