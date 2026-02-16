use assert_matches::assert_matches;
use pretty_assertions::assert_eq;
use std::time::Duration;
use time::{Date, Month, PrimitiveDateTime, Time, UtcOffset};

#[tokio::test]
async fn control_handlers_apply_commands_against_fake_session() -> anyhow::Result<()> {
    let fake_args = idm::FakeArgs::builder()
        .scan_fixture("hci0|AA:BB:CC|IDM-Clock|-43")?
        .build();
    let client = idm::fake_hardware_client(fake_args);
    let session = client.connect_first_device("IDM-").await?;

    idm::PowerHandler::set_power(&session, idm::ScreenPower::Off).await?;
    idm::PowerHandler::set_power(&session, idm::ScreenPower::On).await?;

    let brightness = idm::Brightness::new(75)?;
    idm::BrightnessHandler::set_brightness(&session, brightness).await?;

    idm::FullscreenColourHandler::set_colour(&session, idm::Rgb::new(0x11, 0x22, 0x33)).await?;

    let timestamp = PrimitiveDateTime::new(
        Date::from_calendar_date(2026, Month::February, 16)?,
        Time::from_hms(9, 30, 45)?,
    )
    .assume_offset(UtcOffset::UTC);
    idm::TimeSyncHandler::sync_time(&session, timestamp).await?;

    session.close().await?;
    Ok(())
}

#[test]
fn brightness_rejects_values_outside_range() {
    let result = idm::Brightness::new(101);
    assert_matches!(
        result,
        Err(idm::BrightnessError::OutOfRange {
            value: 101,
            min: 0,
            max: 100,
        })
    );
}

#[tokio::test]
async fn text_upload_handler_writes_expected_payload_size() -> anyhow::Result<()> {
    let fake_args = idm::FakeArgs::builder()
        .scan_fixture("hci0|AA:BB:CC|IDM-Clock|-43")?
        .build();
    let client = idm::fake_hardware_client(fake_args);
    let session = client.connect_first_device("IDM-").await?;

    let receipt =
        idm::TextUploadHandler::upload(&session, idm::TextUploadRequest::new("Hi")).await?;

    assert_eq!(166, receipt.bytes_written());
    assert_eq!(1, receipt.chunks_written());
    session.close().await?;
    Ok(())
}

#[tokio::test]
async fn text_upload_handler_supports_notify_ack_pacing() -> anyhow::Result<()> {
    let fake_args = idm::FakeArgs::builder()
        .scan_fixture("hci0|AA:BB:CC|IDM-Clock|-43")?
        .notifications("0500010001")?
        .build();
    let client = idm::fake_hardware_client(fake_args);
    let session = client.connect_first_device("IDM-").await?;

    let request = idm::TextUploadRequest::new("Hi").with_pacing(idm::UploadPacing::NotifyAck {
        timeout: Duration::from_millis(250),
    });
    let receipt = idm::TextUploadHandler::upload(&session, request).await?;

    assert_eq!(1, receipt.chunks_written());
    session.close().await?;
    Ok(())
}

#[tokio::test]
async fn text_upload_rejects_unresolved_text_path_routing_profile() -> anyhow::Result<()> {
    let fake_args = idm::FakeArgs::builder()
        .scan_fixture("hci0|AA:BB:CC|IDM-Unknown|-43|5452007042010200090920002000")?
        .initial_read("09000180020A016300")?
        .build();
    let client = idm::fake_hardware_client(fake_args);
    let session = client.connect_first_device("IDM-").await?;

    let result = idm::TextUploadHandler::upload(&session, idm::TextUploadRequest::new("Hi")).await;
    assert_matches!(
        result,
        Err(idm::ProtocolError::TextUpload(error))
            if matches!(*error, idm::TextUploadError::UnresolvedTextPath)
    );

    session.close().await?;
    Ok(())
}
