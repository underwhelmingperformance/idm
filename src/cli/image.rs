use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Args;
use serde::Serialize;
use tracing::instrument;

use crate::cli::OutputFormat;
use crate::hw::HardwareClient;
use crate::{
    GifUploadHandler, GifUploadRequest, ImagePreprocessor, ImageUploadHandler, ImageUploadRequest,
    PreparedImageUpload, SessionHandler,
};

/// JSON result emitted by `image` command.
#[derive(Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum ImageResult {
    Image {
        media_type: String,
        bytes_written: usize,
        chunks_written: usize,
        logical_chunks_sent: usize,
    },
}

/// Arguments for top-level `image` upload command.
#[derive(Debug, Args)]
pub struct ImageArgs {
    /// Path to a source image file.
    image_file: PathBuf,
}

impl ImageArgs {
    /// Creates image-upload arguments.
    ///
    /// ```
    /// use std::path::Path;
    /// use std::path::PathBuf;
    ///
    /// use idm::ImageArgs;
    ///
    /// let args = ImageArgs::new(PathBuf::from("photo.jpg"));
    /// assert_eq!(Path::new("photo.jpg"), args.path());
    /// ```
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            image_file: path.into(),
        }
    }

    /// Returns the selected image file path.
    ///
    /// ```
    /// use std::path::Path;
    /// use std::path::PathBuf;
    ///
    /// use idm::ImageArgs;
    ///
    /// let args = ImageArgs::new(PathBuf::from("photo.jpg"));
    /// assert_eq!(Path::new("photo.jpg"), args.path());
    /// ```
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.image_file
    }
}

/// Executes the top-level `image` command.
#[instrument(skip(client, args, out), level = "info", fields(?output_format))]
pub(crate) async fn run<W>(
    client: Box<dyn HardwareClient>,
    args: &ImageArgs,
    out: &mut W,
    output_format: OutputFormat,
) -> Result<()>
where
    W: io::Write,
{
    let session = SessionHandler::new(client).connect_first().await?;

    let command_result = run_with_session(&session, args, out, output_format).await;
    let close_result = session.close().await;

    if let Err(error) = close_result {
        if command_result.is_ok() {
            return Err(error.into());
        }
        tracing::trace!(?error, "failed to close image session cleanly");
    }

    command_result
}

#[instrument(skip(session, args, out), level = "debug", fields(?output_format))]
async fn run_with_session<W>(
    session: &crate::DeviceSession,
    args: &ImageArgs,
    out: &mut W,
    output_format: OutputFormat,
) -> Result<()>
where
    W: io::Write,
{
    let panel_dimensions = session
        .device_profile()
        .panel_dimensions()
        .context("cannot upload image because panel dimensions are unresolved for this device")?;
    let source_bytes = std::fs::read(args.path())
        .with_context(|| format!("failed to read image file `{}`", args.path().display()))?;
    let prepared = ImagePreprocessor::prepare_for_upload(&source_bytes, panel_dimensions)
        .with_context(|| format!("failed to prepare image file `{}`", args.path().display()))?;

    match prepared {
        PreparedImageUpload::Still(still) => {
            let receipt =
                ImageUploadHandler::upload(session, ImageUploadRequest::new(still.into_frame()))
                    .await?;
            match output_format {
                OutputFormat::Pretty => {
                    writeln!(
                        out,
                        "Uploaded image payload: {} bytes in {} chunk(s) across {} logical chunk(s)",
                        receipt.bytes_written(),
                        receipt.chunks_written(),
                        receipt.logical_chunks_sent(),
                    )?;
                }
                OutputFormat::Json => {
                    write_json_line(
                        out,
                        &ImageResult::Image {
                            media_type: "image".to_string(),
                            bytes_written: receipt.bytes_written(),
                            chunks_written: receipt.chunks_written(),
                            logical_chunks_sent: receipt.logical_chunks_sent(),
                        },
                    )?;
                }
            }
        }
        PreparedImageUpload::Gif(gif) => {
            let receipt = GifUploadHandler::upload(session, GifUploadRequest::new(gif)).await?;
            match output_format {
                OutputFormat::Pretty => {
                    if receipt.cached() {
                        writeln!(
                            out,
                            "Uploaded GIF payload: {} bytes in {} chunk(s); device cache hit",
                            receipt.bytes_written(),
                            receipt.chunks_written(),
                        )?;
                    } else {
                        writeln!(
                            out,
                            "Uploaded GIF payload: {} bytes in {} chunk(s) across {} logical chunk(s)",
                            receipt.bytes_written(),
                            receipt.chunks_written(),
                            receipt.logical_chunks_sent(),
                        )?;
                    }
                }
                OutputFormat::Json => {
                    write_json_line(
                        out,
                        &ImageResult::Image {
                            media_type: "gif".to_string(),
                            bytes_written: receipt.bytes_written(),
                            chunks_written: receipt.chunks_written(),
                            logical_chunks_sent: receipt.logical_chunks_sent(),
                        },
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn write_json_line(out: &mut impl io::Write, value: &impl Serialize) -> Result<()> {
    serde_json::to_writer_pretty(&mut *out, value)?;
    writeln!(out)?;
    Ok(())
}
