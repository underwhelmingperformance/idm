use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
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
    /// Writes the preprocessed GIF payload to this path before upload.
    #[arg(long, value_name = "PATH")]
    save_gif: Option<PathBuf>,
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
            save_gif: None,
        }
    }

    /// Sets an output path for saving preprocessed GIF bytes.
    ///
    /// ```
    /// use std::path::Path;
    /// use std::path::PathBuf;
    ///
    /// use idm::ImageArgs;
    ///
    /// let args = ImageArgs::new(PathBuf::from("photo.gif"))
    ///     .with_save_gif(PathBuf::from("normalised.gif"));
    /// assert_eq!(
    ///     Some(Path::new("normalised.gif")),
    ///     args.save_gif_path(),
    /// );
    /// ```
    #[must_use]
    pub fn with_save_gif(mut self, path: impl Into<PathBuf>) -> Self {
        self.save_gif = Some(path.into());
        self
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

    /// Returns the optional preprocessed GIF output path.
    ///
    /// ```
    /// use std::path::Path;
    /// use std::path::PathBuf;
    ///
    /// use idm::ImageArgs;
    ///
    /// let args = ImageArgs::new(PathBuf::from("photo.gif"))
    ///     .with_save_gif(PathBuf::from("normalised.gif"));
    /// assert_eq!(
    ///     Some(Path::new("normalised.gif")),
    ///     args.save_gif_path(),
    /// );
    /// ```
    #[must_use]
    pub fn save_gif_path(&self) -> Option<&Path> {
        self.save_gif.as_deref()
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
            if args.save_gif_path().is_some() {
                bail!("cannot use `--save-gif` because input normalised to a still image payload");
            }
            let request = ImageUploadRequest::new(still.into_frame());
            let receipt = ImageUploadHandler::upload(session, request).await?;
            match output_format {
                OutputFormat::Pretty => {}
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
            if let Some(path) = args.save_gif_path() {
                save_preprocessed_gif(path, gif.payload())?;
            }
            let request = GifUploadRequest::new(gif);
            let receipt = GifUploadHandler::upload(session, request).await?;
            match output_format {
                OutputFormat::Pretty => {}
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

fn save_preprocessed_gif(path: &Path, payload: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create parent directory for preprocessed gif `{}`",
                    path.display()
                )
            })?;
        }
    }
    std::fs::write(path, payload).with_context(|| {
        format!(
            "failed to write preprocessed gif payload to `{}`",
            path.display()
        )
    })?;
    tracing::info!(path = %path.display(), bytes = payload.len(), "saved preprocessed gif payload");
    Ok(())
}
