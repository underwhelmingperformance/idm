use assert_matches::assert_matches;
use pretty_assertions::assert_eq;
use std::time::Duration;
use time::{Date, Month, PrimitiveDateTime, Time, UtcOffset};

const FAKE_SCAN_64X64: &str = "hci0|AA:BB:CC|IDM-Clock|-43|5452007004010200010520002000";

fn tiny_gif_payload() -> Vec<u8> {
    vec![
        0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00,
        0x00, 0xFF, 0xFF, 0xFF, 0x21, 0xF9, 0x04, 0x01, 0x00, 0x00, 0x00, 0x00, 0x2C, 0x00, 0x00,
        0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02, 0x44, 0x01, 0x00, 0x3B,
    ]
}

fn gif_payload_with_padding(extra_bytes: usize) -> Vec<u8> {
    let mut payload = tiny_gif_payload();
    payload.extend(std::iter::repeat_n(0x00, extra_bytes));
    payload
}

fn image_request_64x64() -> anyhow::Result<idm::ImageUploadRequest> {
    let dimensions = idm::PanelDimensions::new(64, 64).expect("64x64 should be valid");
    let mut payload = Vec::with_capacity(64 * 64 * 3);
    for _ in 0..(64 * 64) {
        payload.extend_from_slice(&[0x11, 0x22, 0x33]);
    }
    let frame = idm::Rgb888Frame::try_from((dimensions, payload))?;
    Ok(idm::ImageUploadRequest::new(frame).with_per_fragment_delay(Duration::ZERO))
}

fn stream_closed_listen_scenario() -> idm::ListenScenario {
    idm::ListenScenario::builder()
        .stream_behaviour(idm::ListenStreamBehaviour::CloseAfterInitialNotifications)
        .build()
}

fn timeout_listen_scenario() -> idm::ListenScenario {
    idm::ListenScenario::builder()
        .auto_advance_interval(Duration::from_secs(1))
        .build()
}

fn stale_listen_scenario(event: idm::NotifyEvent, count: usize) -> idm::ListenScenario {
    let notifications = (0..count)
        .map(|_| idm::ListenNotification::Event(event.clone()))
        .collect();
    idm::ListenScenario::builder()
        .notifications(notifications)
        .build()
}

#[tokio::test]
async fn control_handlers_apply_commands_against_fake_session() -> anyhow::Result<()> {
    let fake_args = idm::FakeArgs::builder()
        .scan("hci0|AA:BB:CC|IDM-Clock|-43")?
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
        .scan("hci0|AA:BB:CC|IDM-Clock|-43")?
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
        .scan("hci0|AA:BB:CC|IDM-Clock|-43")?
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
        .scan("hci0|AA:BB:CC|IDM-Unknown|-43|5452007042010200090920002000")?
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
        .scan("hci0|AA:BB:CC|IDM-Clock|-43")?
        .gif(
            idm::GifScenario::builder()
                .first_chunk(idm::AckAction::Finished)
                .build(),
        )
        .build();
    let client = idm::fake_hardware_client(fake_args);
    let session = client.connect_first_device("IDM-").await?;

    let mut payload_bytes = tiny_gif_payload();
    payload_bytes.extend(std::iter::repeat_n(0x00, 5000));
    let payload = idm::GifAnimation::try_from(payload_bytes)?;
    let request = idm::GifUploadRequest::new(payload).with_per_fragment_delay(Duration::ZERO);
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
        .scan("hci0|AA:BB:CC|IDM-Clock|-43")?
        .gif(
            idm::GifScenario::builder()
                .first_chunk(idm::AckAction::Error(0x02))
                .build(),
        )
        .build();
    let client = idm::fake_hardware_client(fake_args);
    let session = client.connect_first_device("IDM-").await?;

    let request = idm::GifUploadRequest::new(idm::GifAnimation::try_from(tiny_gif_payload())?)
        .with_per_fragment_delay(Duration::ZERO);
    let result = idm::GifUploadHandler::upload(&session, request).await;

    assert_matches!(
        result,
        Err(idm::ProtocolError::GifUpload(error))
            if matches!(*error, idm::GifUploadError::TransferRejected { status: 0x02 })
    );

    session.close().await?;
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn text_upload_handler_times_out_when_ack_is_missing() -> anyhow::Result<()> {
    let fake_args = idm::FakeArgs::builder()
        .scan("hci0|AA:BB:CC|IDM-Clock|-43")?
        .listen(timeout_listen_scenario())
        .text(
            idm::TextScenario::builder()
                .first_chunk(idm::AckAction::NoAck)
                .build(),
        )
        .build();
    let client = idm::fake_hardware_client(fake_args);
    let session = client.connect_first_device("IDM-").await?;

    let request = idm::TextUploadRequest::new("Hi").with_pacing(idm::UploadPacing::NotifyAck {
        timeout: Duration::from_secs(5),
    });
    let result = idm::TextUploadHandler::upload(&session, request).await;

    assert_matches!(
        result,
        Err(idm::ProtocolError::TextUpload(error))
            if matches!(*error, idm::TextUploadError::NotifyAckTimeout { .. })
    );
    session.close().await?;
    Ok(())
}

#[tokio::test]
async fn text_upload_handler_surfaces_stream_closure_as_missing_ack() -> anyhow::Result<()> {
    let fake_args = idm::FakeArgs::builder()
        .scan("hci0|AA:BB:CC|IDM-Clock|-43")?
        .listen(stream_closed_listen_scenario())
        .build();
    let client = idm::fake_hardware_client(fake_args);
    let session = client.connect_first_device("IDM-").await?;

    let request = idm::TextUploadRequest::new("Hi").with_pacing(idm::UploadPacing::NotifyAck {
        timeout: Duration::from_millis(50),
    });
    let result = idm::TextUploadHandler::upload(&session, request).await;

    assert_matches!(
        result,
        Err(idm::ProtocolError::TextUpload(error))
            if matches!(*error, idm::TextUploadError::MissingNotifyAck)
    );

    session.close().await?;
    Ok(())
}

#[tokio::test]
async fn text_upload_handler_rejects_unexpected_ack_event() -> anyhow::Result<()> {
    let fake_args = idm::FakeArgs::builder()
        .scan("hci0|AA:BB:CC|IDM-Clock|-43")?
        .listen(stale_listen_scenario(
            idm::NotifyEvent::NextPackage(idm::TransferFamily::Gif),
            1,
        ))
        .build();
    let client = idm::fake_hardware_client(fake_args);
    let session = client.connect_first_device("IDM-").await?;

    let request = idm::TextUploadRequest::new("Hi").with_pacing(idm::UploadPacing::NotifyAck {
        timeout: Duration::from_millis(50),
    });
    let result = idm::TextUploadHandler::upload(&session, request).await;

    assert_matches!(
        result,
        Err(idm::ProtocolError::TextUpload(error))
            if matches!(*error, idm::TextUploadError::UnexpectedNotifyEvent)
    );

    session.close().await?;
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn gif_upload_handler_times_out_when_ack_is_missing() -> anyhow::Result<()> {
    let fake_args = idm::FakeArgs::builder()
        .scan("hci0|AA:BB:CC|IDM-Clock|-43")?
        .listen(timeout_listen_scenario())
        .gif(
            idm::GifScenario::builder()
                .first_chunk(idm::AckAction::NoAck)
                .build(),
        )
        .build();
    let client = idm::fake_hardware_client(fake_args);
    let session = client.connect_first_device("IDM-").await?;

    let request = idm::GifUploadRequest::new(idm::GifAnimation::try_from(tiny_gif_payload())?)
        .with_per_fragment_delay(Duration::ZERO)
        .with_ack_timeout(Duration::from_secs(5));
    let result = idm::GifUploadHandler::upload(&session, request).await;

    assert_matches!(
        result,
        Err(idm::ProtocolError::GifUpload(error))
            if matches!(*error, idm::GifUploadError::NotifyAckTimeout { .. })
    );
    session.close().await?;
    Ok(())
}

#[tokio::test]
async fn gif_upload_handler_surfaces_stream_closure_as_missing_ack() -> anyhow::Result<()> {
    let fake_args = idm::FakeArgs::builder()
        .scan("hci0|AA:BB:CC|IDM-Clock|-43")?
        .listen(stream_closed_listen_scenario())
        .build();
    let client = idm::fake_hardware_client(fake_args);
    let session = client.connect_first_device("IDM-").await?;

    let request = idm::GifUploadRequest::new(idm::GifAnimation::try_from(tiny_gif_payload())?)
        .with_per_fragment_delay(Duration::ZERO);
    let result = idm::GifUploadHandler::upload(&session, request).await;

    assert_matches!(
        result,
        Err(idm::ProtocolError::GifUpload(error))
            if matches!(*error, idm::GifUploadError::MissingNotifyAck)
    );

    session.close().await?;
    Ok(())
}

#[tokio::test]
async fn gif_upload_handler_rejects_unexpected_ack_event() -> anyhow::Result<()> {
    let fake_args = idm::FakeArgs::builder()
        .scan("hci0|AA:BB:CC|IDM-Clock|-43")?
        .listen(stale_listen_scenario(
            idm::NotifyEvent::NextPackage(idm::TransferFamily::Text),
            9,
        ))
        .build();
    let client = idm::fake_hardware_client(fake_args);
    let session = client.connect_first_device("IDM-").await?;

    let request = idm::GifUploadRequest::new(idm::GifAnimation::try_from(tiny_gif_payload())?)
        .with_per_fragment_delay(Duration::ZERO);
    let result = idm::GifUploadHandler::upload(&session, request).await;

    assert_matches!(
        result,
        Err(idm::ProtocolError::GifUpload(error))
            if matches!(*error, idm::GifUploadError::UnexpectedNotifyEvent)
    );

    session.close().await?;
    Ok(())
}

#[tokio::test]
async fn gif_upload_handler_surfaces_premature_finish_on_non_final_chunk() -> anyhow::Result<()> {
    let fake_args = idm::FakeArgs::builder()
        .scan("hci0|AA:BB:CC|IDM-Clock|-43")?
        .gif(
            idm::GifScenario::builder()
                .non_final_chunk(idm::AckAction::Finished)
                .build(),
        )
        .build();
    let client = idm::fake_hardware_client(fake_args);
    let session = client.connect_first_device("IDM-").await?;

    let payload = idm::GifAnimation::try_from(gif_payload_with_padding(9000))?;
    let request = idm::GifUploadRequest::new(payload).with_per_fragment_delay(Duration::ZERO);
    let result = idm::GifUploadHandler::upload(&session, request).await;

    assert_matches!(
        result,
        Err(idm::ProtocolError::GifUpload(error))
            if matches!(*error, idm::GifUploadError::PrematureFinish { .. })
    );

    session.close().await?;
    Ok(())
}

#[tokio::test]
async fn gif_upload_handler_surfaces_non_final_chunk_rejection() -> anyhow::Result<()> {
    let fake_args = idm::FakeArgs::builder()
        .scan("hci0|AA:BB:CC|IDM-Clock|-43")?
        .gif(
            idm::GifScenario::builder()
                .non_final_chunk(idm::AckAction::Error(0x07))
                .build(),
        )
        .build();
    let client = idm::fake_hardware_client(fake_args);
    let session = client.connect_first_device("IDM-").await?;

    let payload = idm::GifAnimation::try_from(gif_payload_with_padding(9000))?;
    let request = idm::GifUploadRequest::new(payload).with_per_fragment_delay(Duration::ZERO);
    let result = idm::GifUploadHandler::upload(&session, request).await;

    assert_matches!(
        result,
        Err(idm::ProtocolError::GifUpload(error))
            if matches!(*error, idm::GifUploadError::TransferRejected { status: 0x07 })
    );

    session.close().await?;
    Ok(())
}

#[tokio::test]
async fn gif_upload_handler_surfaces_last_chunk_rejection() -> anyhow::Result<()> {
    let fake_args = idm::FakeArgs::builder()
        .scan("hci0|AA:BB:CC|IDM-Clock|-43")?
        .gif(
            idm::GifScenario::builder()
                .last_chunk(idm::AckAction::Error(0x11))
                .build(),
        )
        .build();
    let client = idm::fake_hardware_client(fake_args);
    let session = client.connect_first_device("IDM-").await?;

    let payload = idm::GifAnimation::try_from(gif_payload_with_padding(5000))?;
    let request = idm::GifUploadRequest::new(payload).with_per_fragment_delay(Duration::ZERO);
    let result = idm::GifUploadHandler::upload(&session, request).await;

    assert_matches!(
        result,
        Err(idm::ProtocolError::GifUpload(error))
            if matches!(*error, idm::GifUploadError::TransferRejected { status: 0x11 })
    );

    session.close().await?;
    Ok(())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn image_upload_handler_times_out_when_ack_is_missing() -> anyhow::Result<()> {
    let fake_args = idm::FakeArgs::builder()
        .scan(FAKE_SCAN_64X64)?
        .listen(timeout_listen_scenario())
        .image(
            idm::ImageScenario::builder()
                .first_chunk(idm::AckAction::NoAck)
                .build(),
        )
        .build();
    let client = idm::fake_hardware_client(fake_args);
    let session = client.connect_first_device("IDM-").await?;

    let request = image_request_64x64()?.with_ack_timeout(Duration::from_secs(5));
    let result = idm::ImageUploadHandler::upload(&session, request).await;

    assert_matches!(
        result,
        Err(idm::ProtocolError::ImageUpload(error))
            if matches!(*error, idm::ImageUploadError::NotifyAckTimeout { .. })
    );
    session.close().await?;
    Ok(())
}

#[tokio::test]
async fn image_upload_handler_surfaces_stream_closure_as_missing_ack() -> anyhow::Result<()> {
    let fake_args = idm::FakeArgs::builder()
        .scan(FAKE_SCAN_64X64)?
        .listen(stream_closed_listen_scenario())
        .build();
    let client = idm::fake_hardware_client(fake_args);
    let session = client.connect_first_device("IDM-").await?;

    let result = idm::ImageUploadHandler::upload(&session, image_request_64x64()?).await;

    assert_matches!(
        result,
        Err(idm::ProtocolError::ImageUpload(error))
            if matches!(*error, idm::ImageUploadError::MissingNotifyAck)
    );

    session.close().await?;
    Ok(())
}

#[tokio::test]
async fn image_upload_handler_rejects_unexpected_ack_event() -> anyhow::Result<()> {
    let fake_args = idm::FakeArgs::builder()
        .scan(FAKE_SCAN_64X64)?
        .listen(stale_listen_scenario(
            idm::NotifyEvent::NextPackage(idm::TransferFamily::Text),
            9,
        ))
        .build();
    let client = idm::fake_hardware_client(fake_args);
    let session = client.connect_first_device("IDM-").await?;

    let result = idm::ImageUploadHandler::upload(&session, image_request_64x64()?).await;

    assert_matches!(
        result,
        Err(idm::ProtocolError::ImageUpload(error))
            if matches!(*error, idm::ImageUploadError::UnexpectedNotifyEvent)
    );

    session.close().await?;
    Ok(())
}

#[tokio::test]
async fn image_upload_handler_surfaces_device_rejection_status() -> anyhow::Result<()> {
    let fake_args = idm::FakeArgs::builder()
        .scan(FAKE_SCAN_64X64)?
        .image(
            idm::ImageScenario::builder()
                .first_chunk(idm::AckAction::Error(0x02))
                .build(),
        )
        .build();
    let client = idm::fake_hardware_client(fake_args);
    let session = client.connect_first_device("IDM-").await?;

    let result = idm::ImageUploadHandler::upload(&session, image_request_64x64()?).await;

    assert_matches!(
        result,
        Err(idm::ProtocolError::ImageUpload(error))
            if matches!(*error, idm::ImageUploadError::TransferRejected { status: 0x02 })
    );

    session.close().await?;
    Ok(())
}

#[tokio::test]
async fn image_upload_handler_surfaces_premature_finish() -> anyhow::Result<()> {
    let fake_args = idm::FakeArgs::builder()
        .scan(FAKE_SCAN_64X64)?
        .image(
            idm::ImageScenario::builder()
                .first_chunk(idm::AckAction::Finished)
                .build(),
        )
        .build();
    let client = idm::fake_hardware_client(fake_args);
    let session = client.connect_first_device("IDM-").await?;

    let result = idm::ImageUploadHandler::upload(&session, image_request_64x64()?).await;

    assert_matches!(
        result,
        Err(idm::ProtocolError::ImageUpload(error))
            if matches!(*error, idm::ImageUploadError::PrematureFinish { .. })
    );

    session.close().await?;
    Ok(())
}

#[tokio::test]
async fn image_upload_handler_surfaces_non_final_chunk_rejection() -> anyhow::Result<()> {
    let fake_args = idm::FakeArgs::builder()
        .scan(FAKE_SCAN_64X64)?
        .image(
            idm::ImageScenario::builder()
                .non_final_chunk(idm::AckAction::Error(0x0A))
                .build(),
        )
        .build();
    let client = idm::fake_hardware_client(fake_args);
    let session = client.connect_first_device("IDM-").await?;

    let result = idm::ImageUploadHandler::upload(&session, image_request_64x64()?).await;

    assert_matches!(
        result,
        Err(idm::ProtocolError::ImageUpload(error))
            if matches!(*error, idm::ImageUploadError::TransferRejected { status: 0x0A })
    );

    session.close().await?;
    Ok(())
}

#[tokio::test]
async fn image_upload_handler_surfaces_last_chunk_rejection() -> anyhow::Result<()> {
    let fake_args = idm::FakeArgs::builder()
        .scan(FAKE_SCAN_64X64)?
        .image(
            idm::ImageScenario::builder()
                .last_chunk(idm::AckAction::Error(0x0B))
                .build(),
        )
        .build();
    let client = idm::fake_hardware_client(fake_args);
    let session = client.connect_first_device("IDM-").await?;

    let result = idm::ImageUploadHandler::upload(&session, image_request_64x64()?).await;

    assert_matches!(
        result,
        Err(idm::ProtocolError::ImageUpload(error))
            if matches!(*error, idm::ImageUploadError::TransferRejected { status: 0x0B })
    );

    session.close().await?;
    Ok(())
}
