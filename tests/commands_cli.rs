use insta::assert_snapshot;
use std::time::{Duration, Instant};

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
    idm::run_with_terminal_client(args, &mut output, &FakeTerminalClient).await?;
    Ok(String::from_utf8(output)?)
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
