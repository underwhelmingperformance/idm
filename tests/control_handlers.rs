use assert_matches::assert_matches;
use pretty_assertions::assert_eq;
use std::time::Duration;
use time::{Date, Month, PrimitiveDateTime, Time, UtcOffset};

fn tiny_gif_payload() -> Vec<u8> {
    vec![
        0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00,
        0x00, 0xFF, 0xFF, 0xFF, 0x21, 0xF9, 0x04, 0x01, 0x00, 0x00, 0x00, 0x00, 0x2C, 0x00, 0x00,
        0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02, 0x44, 0x01, 0x00, 0x3B,
    ]
}

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

    assert_eq!(70, receipt.bytes_written());
    assert_eq!(1, receipt.chunks_written());
    session.close().await?;
    Ok(())
}

#[tokio::test]
async fn text_upload_handler_supports_notify_ack_pacing() -> anyhow::Result<()> {
    let fake_args = idm::FakeArgs::builder()
        .scan_fixture("hci0|AA:BB:CC|IDM-Clock|-43")?
        .notifications("0500030001")?
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

#[tokio::test]
async fn gif_upload_handler_reports_cache_hit_on_first_chunk_finish() -> anyhow::Result<()> {
    let fake_args = idm::FakeArgs::builder()
        .scan_fixture("hci0|AA:BB:CC|IDM-Clock|-43")?
        .notifications("0500010003")?
        .build();
    let client = idm::fake_hardware_client(fake_args);
    let session = client.connect_first_device("IDM-").await?;

    let mut payload_bytes = tiny_gif_payload();
    payload_bytes.extend(std::iter::repeat_n(0x00, 5000));
    let payload = idm::GifAnimation::try_from(payload_bytes)?;
    let request = idm::GifUploadRequest::new(payload)
        .with_per_fragment_delay(Duration::ZERO)
        .with_ack_timeout(Duration::from_millis(250));
    let receipt = idm::GifUploadHandler::upload(&session, request).await?;

    assert_eq!(true, receipt.cached());
    assert_eq!(1, receipt.logical_chunks_sent());
    assert_eq!(4112, receipt.bytes_written());
    assert_eq!(9, receipt.chunks_written());

    session.close().await?;
    Ok(())
}

#[tokio::test]
async fn gif_upload_handler_surfaces_device_rejection_status() -> anyhow::Result<()> {
    let fake_args = idm::FakeArgs::builder()
        .scan_fixture("hci0|AA:BB:CC|IDM-Clock|-43")?
        .notifications("0500010002")?
        .build();
    let client = idm::fake_hardware_client(fake_args);
    let session = client.connect_first_device("IDM-").await?;

    let request = idm::GifUploadRequest::new(idm::GifAnimation::try_from(tiny_gif_payload())?)
        .with_per_fragment_delay(Duration::ZERO)
        .with_ack_timeout(Duration::from_millis(250));
    let result = idm::GifUploadHandler::upload(&session, request).await;

    assert_matches!(
        result,
        Err(idm::ProtocolError::GifUpload(error))
            if matches!(*error, idm::GifUploadError::TransferRejected { status: 0x02 })
    );

    session.close().await?;
    Ok(())
}
