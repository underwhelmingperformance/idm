use insta::assert_snapshot;
use std::time::{Duration, Instant};

use clap::Parser;
use clap::error::ErrorKind;
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
    idm::run_with_clients(command, &mut output, &FakeTerminalClient, hardware_client).await?;
    Ok(String::from_utf8(output)?)
}

async fn run_with_argv<const N: usize>(argv: [&str; N]) -> anyhow::Result<String> {
    let parsed_args = idm::Args::try_parse_from(argv)?;
    run_with_parsed_args(parsed_args).await
}

#[tokio::test]
async fn inspect_command_prints_gatt_details_from_fake_backend() -> anyhow::Result<()> {
    let fake = idm::FakeArgs::builder()
        .scan_fixture("hci1|00:11:22|Speaker|-65;hci0|AA:BB:CC|IDM-Clock|-43")?
        .build();
    let args = idm::Args::new(idm::Command::Inspect).with_fake(fake);

    let stdout = run_with_parsed_args(args).await?;
    assert_snapshot!("inspect_command_stdout", stdout.trim_end());

    Ok(())
}

#[tokio::test]
async fn listen_command_reads_once_then_streams_notifications() -> anyhow::Result<()> {
    let fake = idm::FakeArgs::builder()
        .scan_fixture("hci0|AA:BB:CC|IDM-Clock|-43")?
        .initial_read("DEADBEEF")?
        .notifications("0500010001,0500010003,AA55")?
        .build();
    let args = idm::Args::new(idm::Command::Listen(idm::ListenArgs::new(Some(2)))).with_fake(fake);

    let stdout = run_with_parsed_args(args).await?;
    assert_snapshot!("listen_command_stdout", stdout.trim_end());

    Ok(())
}

#[test]
fn inspect_command_fails_for_invalid_fixture() {
    let result = idm::FakeArgs::builder().scan_fixture("invalid-record");
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
    let fake = idm::FakeArgs::builder()
        .scan_fixture("hci0|AA:BB:CC|IDM-Clock|-43")?
        .discovery_delay(Duration::from_millis(40))
        .build();
    let args = idm::Args::new(idm::Command::Inspect).with_fake(fake);

    let started_at = Instant::now();
    let _ = run_with_parsed_args(args).await?;

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

    assert_eq!("Applied power state: on", stdout.trim_end());
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

    assert_eq!("Applied brightness: 80", stdout.trim_end());
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

    assert_eq!("Applied fullscreen colour: #112233", stdout.trim_end());
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

    assert_eq!("Synced time (UTC unix): 1700000000", stdout.trim_end());
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

    assert_eq!(
        "Uploaded text payload: 70 bytes in 1 chunk(s)",
        stdout.trim_end()
    );
    Ok(())
}
