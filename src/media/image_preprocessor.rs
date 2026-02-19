use std::collections::{HashMap, HashSet};
use std::io::Cursor;

use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView};
use thiserror::Error;

use crate::hw::PanelDimensions;

use super::{GifAnimation, Rgb888Frame};

const MAX_GIF_FRAMES: usize = 64;
const MIN_GIF_DELAY_CENTISECONDS: u16 = 1;
const GIF_QUANTISATION_SPEED: i32 = 1;
const MAX_GIF_PALETTE_COLOURS: usize = 256;

/// Errors returned when preparing an image for panel upload.
#[derive(Debug, Error)]
pub enum ImagePreparationError {
    /// The source bytes are not a supported image format.
    #[error("failed to detect image format from source bytes")]
    UnknownFormat(#[source] image::ImageError),
    /// The source image failed to decode.
    #[error("failed to decode source image")]
    Decode(#[source] image::ImageError),
    /// The source GIF stream failed to decode.
    #[error("failed to decode source gif")]
    GifDecode { source: gif::DecodingError },
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
        let source_gif = GifAnimation::try_from(source_bytes)?;
        if source_gif.dimensions() == panel_dimensions {
            return Ok(source_gif);
        }

        let mut decoder = gif::DecodeOptions::new();
        decoder.set_color_output(gif::ColorOutput::Indexed);
        let mut reader = decoder
            .read_info(Cursor::new(source_bytes))
            .map_err(|source| ImagePreparationError::GifDecode { source })?;
        let global_palette = reader.global_palette().map(ToOwned::to_owned);
        let source_width = u32::from(reader.width());
        let source_height = u32::from(reader.height());
        let mut composite_canvas = image::RgbaImage::from_pixel(
            source_width,
            source_height,
            image::Rgba([0x00, 0x00, 0x00, 0xFF]),
        );
        let orientation = exif_orientation(source_bytes);
        let panel_width = panel_dimensions.width();
        let panel_height = panel_dimensions.height();
        let mut transformed_frames = Vec::new();

        while transformed_frames.len() < MAX_GIF_FRAMES {
            let Some(frame) = reader
                .read_next_frame()
                .map_err(|source| ImagePreparationError::GifDecode { source })?
            else {
                break;
            };
            composite_indexed_frame(&mut composite_canvas, frame, global_palette.as_deref());
            let dynamic = DynamicImage::ImageRgba8(composite_canvas.clone());
            let oriented = apply_orientation(dynamic, orientation);
            let padded = resize_and_pad_rgba(oriented, panel_dimensions);
            transformed_frames.push(PreparedGifFrame {
                rgba_pixels: padded.into_raw(),
                delay_centiseconds: frame.delay.max(MIN_GIF_DELAY_CENTISECONDS),
            });

            if frame.dispose == gif::DisposalMethod::Background {
                clear_rect(
                    &mut composite_canvas,
                    u32::from(frame.left),
                    u32::from(frame.top),
                    u32::from(frame.width),
                    u32::from(frame.height),
                );
            }
        }
        if transformed_frames.is_empty() {
            return Err(ImagePreparationError::GifHasNoFrames);
        }

        let transformed_payload =
            encode_gif_frames_with_shared_palette(panel_width, panel_height, &transformed_frames)?;
        let transformed_payload = strip_empty_global_palette(transformed_payload);
        Ok(GifAnimation::try_from(transformed_payload)?)
    }
}

struct PreparedGifFrame {
    rgba_pixels: Vec<u8>,
    delay_centiseconds: u16,
}

enum SharedGifPaletteIndexer {
    Exact(HashMap<[u8; 4], u8>),
    Quantised(color_quant::NeuQuant),
}

struct SharedGifPalette {
    palette_bytes: Vec<u8>,
    indexer: SharedGifPaletteIndexer,
}

impl SharedGifPalette {
    fn build(frames: &[PreparedGifFrame]) -> Self {
        let mut unique_colours = HashSet::new();
        for frame in frames {
            for rgba_pixel in frame.rgba_pixels.chunks_exact(4) {
                unique_colours.insert([rgba_pixel[0], rgba_pixel[1], rgba_pixel[2], rgba_pixel[3]]);
                if unique_colours.len() > MAX_GIF_PALETTE_COLOURS {
                    return Self::build_quantised(frames);
                }
            }
        }

        let mut colours: Vec<[u8; 4]> = unique_colours.into_iter().collect();
        colours.sort_unstable();
        let palette_bytes = colours
            .iter()
            .flat_map(|colour| [colour[0], colour[1], colour[2]])
            .collect();
        let mut lookup = HashMap::with_capacity(colours.len());
        for (index, colour) in colours.into_iter().enumerate() {
            let index = u8::try_from(index)
                .expect("exact palette length is bounded by MAX_GIF_PALETTE_COLOURS");
            lookup.insert(colour, index);
        }

        Self {
            palette_bytes,
            indexer: SharedGifPaletteIndexer::Exact(lookup),
        }
    }

    fn build_quantised(frames: &[PreparedGifFrame]) -> Self {
        let sample_size = frames.iter().map(|frame| frame.rgba_pixels.len()).sum();
        let mut sampled_pixels = Vec::with_capacity(sample_size);
        for frame in frames {
            sampled_pixels.extend_from_slice(&frame.rgba_pixels);
        }

        let quantiser = color_quant::NeuQuant::new(
            GIF_QUANTISATION_SPEED,
            MAX_GIF_PALETTE_COLOURS,
            &sampled_pixels,
        );
        let palette_bytes = quantiser.color_map_rgb();
        Self {
            palette_bytes,
            indexer: SharedGifPaletteIndexer::Quantised(quantiser),
        }
    }

    fn palette_bytes(&self) -> &[u8] {
        &self.palette_bytes
    }

    fn index_pixels(&self, rgba_pixels: &[u8]) -> Vec<u8> {
        match &self.indexer {
            SharedGifPaletteIndexer::Exact(lookup) => rgba_pixels
                .chunks_exact(4)
                .map(|rgba_pixel| {
                    let rgba = [rgba_pixel[0], rgba_pixel[1], rgba_pixel[2], rgba_pixel[3]];
                    lookup.get(&rgba).copied().unwrap_or_default()
                })
                .collect(),
            SharedGifPaletteIndexer::Quantised(quantiser) => rgba_pixels
                .chunks_exact(4)
                .map(|rgba_pixel| quantiser.index_of(rgba_pixel) as u8)
                .collect(),
        }
    }
}

fn encode_gif_frames_with_shared_palette(
    panel_width: u16,
    panel_height: u16,
    frames: &[PreparedGifFrame],
) -> Result<Vec<u8>, ImagePreparationError> {
    let shared_palette = SharedGifPalette::build(frames);
    let frame_palette = shared_palette.palette_bytes().to_vec();
    let mut transformed_payload = Vec::new();
    {
        let mut encoder =
            gif::Encoder::new(&mut transformed_payload, panel_width, panel_height, &[])
                .map_err(|source| ImagePreparationError::GifEncode { source })?;
        encoder
            .set_repeat(gif::Repeat::Infinite)
            .map_err(|source| ImagePreparationError::GifEncode { source })?;

        for frame in frames {
            let indexed_pixels = shared_palette.index_pixels(&frame.rgba_pixels);
            let mut encoded_frame = gif::Frame::from_palette_pixels(
                panel_width,
                panel_height,
                indexed_pixels,
                frame_palette.clone(),
                None,
            );
            encoded_frame.delay = frame.delay_centiseconds;
            encoded_frame.dispose = gif::DisposalMethod::Background;
            encoder
                .write_frame(&encoded_frame)
                .map_err(|source| ImagePreparationError::GifEncode { source })?;
        }
    }
    Ok(transformed_payload)
}

fn strip_empty_global_palette(mut payload: Vec<u8>) -> Vec<u8> {
    const LOGICAL_SCREEN_DESCRIPTOR_LEN: usize = 13;
    const GLOBAL_COLOR_TABLE_FLAG: u8 = 0x80;
    const GLOBAL_COLOR_TABLE_SIZE_MASK: u8 = 0x07;

    if payload.len() < LOGICAL_SCREEN_DESCRIPTOR_LEN {
        return payload;
    }

    let packed = payload[10];
    if packed & GLOBAL_COLOR_TABLE_FLAG == 0 {
        return payload;
    }

    let table_size_code = usize::from(packed & GLOBAL_COLOR_TABLE_SIZE_MASK);
    let entries = 1usize << (table_size_code + 1);
    let table_len = entries.saturating_mul(3);
    let table_start = LOGICAL_SCREEN_DESCRIPTOR_LEN;
    let table_end = table_start.saturating_add(table_len);
    if table_end > payload.len() {
        return payload;
    }

    if payload[table_start..table_end]
        .iter()
        .any(|byte| *byte != 0)
    {
        return payload;
    }

    payload[10] = packed & !GLOBAL_COLOR_TABLE_FLAG;
    payload.drain(table_start..table_end);
    payload
}

fn composite_indexed_frame(
    canvas: &mut image::RgbaImage,
    frame: &gif::Frame<'_>,
    global_palette: Option<&[u8]>,
) {
    let palette = frame.palette.as_deref().or(global_palette);
    let Some(palette_bytes) = palette else {
        return;
    };

    let frame_width = usize::from(frame.width);
    for y in 0..frame.height {
        for x in 0..frame.width {
            let row_offset = usize::from(y) * frame_width;
            let pixel_index = row_offset + usize::from(x);
            let colour_index = frame.buffer[pixel_index];
            if frame.transparent == Some(colour_index) {
                continue;
            }
            let palette_offset = usize::from(colour_index) * 3;
            if palette_offset + 2 >= palette_bytes.len() {
                continue;
            }
            let target_x = u32::from(frame.left) + u32::from(x);
            let target_y = u32::from(frame.top) + u32::from(y);
            if target_x >= canvas.width() || target_y >= canvas.height() {
                continue;
            }
            canvas.put_pixel(
                target_x,
                target_y,
                image::Rgba([
                    palette_bytes[palette_offset],
                    palette_bytes[palette_offset + 1],
                    palette_bytes[palette_offset + 2],
                    0xFF,
                ]),
            );
        }
    }
}

fn clear_rect(canvas: &mut image::RgbaImage, left: u32, top: u32, width: u32, height: u32) {
    if width == 0 || height == 0 {
        return;
    }

    let x_end = left.saturating_add(width).min(canvas.width());
    let y_end = top.saturating_add(height).min(canvas.height());
    for y in top..y_end {
        for x in left..x_end {
            canvas.put_pixel(x, y, image::Rgba([0x00, 0x00, 0x00, 0xFF]));
        }
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
    image::imageops::overlay(&mut canvas, &resized, offset_x, offset_y);
    canvas
}

#[cfg(test)]
mod tests {
    use image::{AnimationDecoder, ImageEncoder};
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

    #[test]
    fn prepare_for_upload_gif_frames_are_opaque_with_background_disposal()
    -> Result<(), Box<dyn std::error::Error>> {
        let panel = PanelDimensions::new(2, 2).expect("2x2 should be valid");
        let source = make_source_gif(1, 0, [0xFF, 0x00, 0x00, 0x00])?;
        let prepared = ImagePreprocessor::prepare_for_upload(&source, panel)?;

        let PreparedImageUpload::Gif(gif) = prepared else {
            panic!("gif should produce gif upload");
        };

        let options = gif::DecodeOptions::new();
        let mut reader = options.read_info(Cursor::new(gif.payload()))?;
        let frame = reader
            .read_next_frame()?
            .expect("re-encoded gif should contain one frame");

        assert_eq!(None, frame.transparent);
        assert_eq!(gif::DisposalMethod::Background, frame.dispose);
        assert_eq!(MIN_GIF_DELAY_CENTISECONDS, frame.delay);
        Ok(())
    }

    #[test]
    fn prepare_for_upload_limits_gif_frame_count() -> Result<(), Box<dyn std::error::Error>> {
        let panel = PanelDimensions::new(2, 2).expect("2x2 should be valid");
        let source = make_source_gif(MAX_GIF_FRAMES + 8, 2, [0x10, 0x20, 0x30, 0xFF])?;
        let prepared = ImagePreprocessor::prepare_for_upload(&source, panel)?;

        let PreparedImageUpload::Gif(gif) = prepared else {
            panic!("gif should produce gif upload");
        };
        assert_eq!(MAX_GIF_FRAMES, gif_frame_count(gif.payload())?);
        Ok(())
    }

    #[test]
    fn prepare_for_upload_preserves_native_panel_gif_bytes()
    -> Result<(), Box<dyn std::error::Error>> {
        let panel = PanelDimensions::new(1, 1).expect("1x1 should be valid");
        let prepared = ImagePreprocessor::prepare_for_upload(&MINIMAL_GIF_1X1, panel)?;

        let PreparedImageUpload::Gif(gif) = prepared else {
            panic!("gif should produce gif upload");
        };
        assert_eq!(MINIMAL_GIF_1X1.as_slice(), gif.payload());
        Ok(())
    }

    #[test]
    fn prepare_for_upload_composites_transparent_gif_delta_frames()
    -> Result<(), Box<dyn std::error::Error>> {
        let panel = PanelDimensions::new(2, 2).expect("2x2 should be valid");
        let source = make_transparent_delta_source_gif()?;
        let prepared = ImagePreprocessor::prepare_for_upload(&source, panel)?;
        let PreparedImageUpload::Gif(gif) = prepared else {
            panic!("gif should produce gif upload");
        };

        let decoder = image::codecs::gif::GifDecoder::new(Cursor::new(gif.payload()))?;
        let frames = decoder.into_frames().collect_frames()?;
        assert_eq!(2, frames.len());

        let first = frames[0].buffer();
        let second = frames[1].buffer();
        let first_unchanged = first.get_pixel(1, 1);
        let second_unchanged = second.get_pixel(1, 1);
        let first_changed = first.get_pixel(0, 0);
        let second_changed = second.get_pixel(0, 0);

        assert_eq!(first_unchanged, second_unchanged);
        assert_ne!(first_changed, second_changed);
        Ok(())
    }

    fn make_source_gif(
        frames: usize,
        delay_centiseconds: u16,
        rgba_pixel: [u8; 4],
    ) -> Result<Vec<u8>, gif::EncodingError> {
        let mut payload = Vec::new();
        {
            let mut encoder = gif::Encoder::new(&mut payload, 1, 1, &[])?;
            encoder.set_repeat(gif::Repeat::Infinite)?;

            for _index in 0..frames {
                let mut rgba = Vec::from(rgba_pixel);
                let mut frame = gif::Frame::from_rgba_speed(1, 1, &mut rgba, 10);
                frame.delay = delay_centiseconds;
                encoder.write_frame(&frame)?;
            }
        }
        Ok(payload)
    }

    fn gif_frame_count(payload: &[u8]) -> Result<usize, gif::DecodingError> {
        let options = gif::DecodeOptions::new();
        let mut reader = options.read_info(Cursor::new(payload))?;
        let mut frames = 0usize;
        while reader.read_next_frame()?.is_some() {
            frames += 1;
        }
        Ok(frames)
    }

    fn make_transparent_delta_source_gif() -> Result<Vec<u8>, gif::EncodingError> {
        let mut payload = Vec::new();
        {
            let mut encoder = gif::Encoder::new(&mut payload, 2, 2, &[])?;
            encoder.set_repeat(gif::Repeat::Infinite)?;

            let mut full_red = vec![
                0xFF, 0x00, 0x00, 0xFF, 0xFF, 0x00, 0x00, 0xFF, 0xFF, 0x00, 0x00, 0xFF, 0xFF, 0x00,
                0x00, 0xFF,
            ];
            let mut frame_one = gif::Frame::from_rgba_speed(2, 2, &mut full_red, 10);
            frame_one.delay = 2;
            encoder.write_frame(&frame_one)?;

            let mut transparent_delta = vec![
                0x00, 0x00, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00,
            ];
            let mut frame_two = gif::Frame::from_rgba_speed(2, 2, &mut transparent_delta, 10);
            frame_two.delay = 2;
            encoder.write_frame(&frame_two)?;
        }
        Ok(payload)
    }
}
