use insta::assert_snapshot;
use std::time::{Duration, Instant};
use std::time::{SystemTime, UNIX_EPOCH};

use clap::Parser;
use clap::error::ErrorKind;
use image::ImageEncoder;
use pretty_assertions::assert_eq;

#[derive(Debug, Default)]
struct FakeTerminalClient;

impl idm::TerminalClient for FakeTerminalClient {
    fn stdout_is_terminal(&self) -> bool {
        false
    }

    fn stderr_is_terminal(&self) -> bool {
        false
    }
}

async fn run_with_parsed_args(args: idm::Args) -> anyhow::Result<String> {
    let mut output = Vec::new();
    let model_resolution = args.model_resolution();
    let (command, maybe_fake_args) = args.into_command_and_fake_args()?;
    let hardware_client = match maybe_fake_args {
        Some(fake_args) => idm::fake_hardware_client(fake_args),
        None => idm::real_hardware_client_with_model_resolution(model_resolution),
    };
    idm::run_with_clients(
        command,
        &mut output,
        &FakeTerminalClient,
        hardware_client,
        idm::OutputFormat::Pretty,
    )
    .await?;
    Ok(String::from_utf8(output)?)
}

async fn run_with_argv<const N: usize>(argv: [&str; N]) -> anyhow::Result<String> {
    let parsed_args = idm::Args::try_parse_from(argv)?;
    run_with_parsed_args(parsed_args).await
}

#[tokio::test]
async fn inspect_command_prints_gatt_details_from_fake_backend() -> anyhow::Result<()> {
    let fake = idm::FakeArgs::builder()
        .scan("hci1|00:11:22|Speaker|-65;hci0|AA:BB:CC|IDM-Clock|-43")?
        .build();
    let args = idm::Args::new(idm::Command::Inspect).with_fake(fake);

    let stdout = run_with_parsed_args(args).await?;
    assert_snapshot!("inspect_command_stdout", stdout.trim_end());

    Ok(())
}

#[tokio::test]
async fn listen_command_reads_once_then_streams_notifications() -> anyhow::Result<()> {
    let fake = idm::FakeArgs::builder()
        .scan("hci0|AA:BB:CC|IDM-Clock|-43")?
        .initial_read("DEADBEEF")?
        .listen(idm::ListenFixture::GifTransferHappyPath)
        .build();
    let args = idm::Args::new(idm::Command::Listen(idm::ListenArgs::new(Some(2)))).with_fake(fake);

    let stdout = run_with_parsed_args(args).await?;
    assert_snapshot!("listen_command_stdout", stdout.trim_end());

    Ok(())
}

#[test]
fn inspect_command_fails_for_invalid_fixture() {
    let result = idm::FakeArgs::builder().scan("invalid-record");
    assert!(matches!(
        result,
        Err(idm::FixtureError::InvalidRecordFieldCount)
    ));
}

#[test]
fn control_brightness_rejects_out_of_range_input() {
    let result = idm::Args::try_parse_from([
        "idm",
        "--fake",
        "--fake-scan",
        "hci0|AA:BB:CC|IDM-Clock|-43",
        "control",
        "brightness",
        "101",
    ]);

    let error = result.expect_err("brightness 101 should fail command parsing");
    assert_eq!(ErrorKind::ValueValidation, error.kind());
}

#[tokio::test]
async fn inspect_command_applies_fake_discovery_delay() -> anyhow::Result<()> {
    let started_at = Instant::now();
    let _ = run_with_argv([
        "idm",
        "--fake",
        "--fake-scan",
        "hci0|AA:BB:CC|IDM-Clock|-43",
        "--fake-discovery-delay",
        "40ms",
        "inspect",
    ])
    .await?;

    assert!(started_at.elapsed() >= Duration::from_millis(40));
    Ok(())
}

#[tokio::test]
async fn control_power_command_applies_state() -> anyhow::Result<()> {
    let stdout = run_with_argv([
        "idm",
        "--fake",
        "--fake-scan",
        "hci0|AA:BB:CC|IDM-Clock|-43",
        "control",
        "power",
        "on",
    ])
    .await?;

    assert_snapshot!("control_power_command_stdout", stdout.trim_end());
    Ok(())
}

#[tokio::test]
async fn control_brightness_command_applies_value() -> anyhow::Result<()> {
    let stdout = run_with_argv([
        "idm",
        "--fake",
        "--fake-scan",
        "hci0|AA:BB:CC|IDM-Clock|-43",
        "control",
        "brightness",
        "80",
    ])
    .await?;

    assert_snapshot!("control_brightness_command_stdout", stdout.trim_end());
    Ok(())
}

#[tokio::test]
async fn control_colour_command_applies_rgb_value() -> anyhow::Result<()> {
    let stdout = run_with_argv([
        "idm",
        "--fake",
        "--fake-scan",
        "hci0|AA:BB:CC|IDM-Clock|-43",
        "control",
        "colour",
        "17",
        "34",
        "51",
    ])
    .await?;

    assert_snapshot!("control_colour_command_stdout", stdout.trim_end());
    Ok(())
}

#[tokio::test]
async fn control_sync_time_command_uses_explicit_unix_timestamp() -> anyhow::Result<()> {
    let stdout = run_with_argv([
        "idm",
        "--fake",
        "--fake-scan",
        "hci0|AA:BB:CC|IDM-Clock|-43",
        "control",
        "sync-time",
        "--unix",
        "1700000000",
    ])
    .await?;

    assert_snapshot!("control_sync_time_command_stdout", stdout.trim_end());
    Ok(())
}

#[tokio::test]
async fn control_text_command_uploads_payload() -> anyhow::Result<()> {
    let stdout = run_with_argv([
        "idm",
        "--fake",
        "--fake-scan",
        "hci0|AA:BB:CC|IDM-Clock|-43",
        "control",
        "text",
        "Hi",
    ])
    .await?;

    assert_snapshot!("control_text_command_stdout", stdout.trim_end());
    Ok(())
}

#[tokio::test]
async fn image_command_uploads_gif_payload() -> anyhow::Result<()> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    let file_path = std::env::temp_dir().join(format!(
        "idm-control-gif-cli-{}-{timestamp}.gif",
        std::process::id()
    ));
    std::fs::write(
        &file_path,
        [
            0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00, 0x00,
            0x00, 0x00, 0xFF, 0xFF, 0xFF, 0x21, 0xF9, 0x04, 0x01, 0x00, 0x00, 0x00, 0x00, 0x2C,
            0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02, 0x44, 0x01, 0x00,
            0x3B,
        ],
    )?;

    let fake = idm::FakeArgs::builder()
        .scan("hci0|AA:BB:CC|IDM-16-Clock|-43")?
        .build();
    let args = idm::Args::new(idm::Command::Image(idm::ImageArgs::new(&file_path))).with_fake(fake);

    let stdout = run_with_parsed_args(args).await?;
    assert_snapshot!("image_command_uploads_gif_stdout", stdout.trim_end());

    std::fs::remove_file(file_path)?;
    Ok(())
}

#[tokio::test]
async fn image_command_uploads_transformed_payload() -> anyhow::Result<()> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    let file_path = std::env::temp_dir().join(format!(
        "idm-image-cli-{}-{timestamp}.png",
        std::process::id()
    ));

    let source = image::RgbaImage::from_pixel(2, 1, image::Rgba([0x11, 0x22, 0x33, 0xFF]));
    let mut encoded = Vec::new();
    image::codecs::png::PngEncoder::new(&mut encoded).write_image(
        source.as_raw(),
        2,
        1,
        image::ExtendedColorType::Rgba8,
    )?;
    std::fs::write(&file_path, encoded)?;

    let fake = idm::FakeArgs::builder()
        .scan("hci0|AA:BB:CC|IDM-16-Clock|-43")?
        .build();
    let args = idm::Args::new(idm::Command::Image(idm::ImageArgs::new(&file_path))).with_fake(fake);

    let stdout = run_with_parsed_args(args).await?;
    assert_snapshot!(
        "image_command_uploads_transformed_stdout",
        stdout.trim_end()
    );

    std::fs::remove_file(file_path)?;
    Ok(())
}
