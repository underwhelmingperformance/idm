use assert_matches::assert_matches;
use pretty_assertions::assert_eq;
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn fake_session_connect_populates_report_metadata() -> anyhow::Result<()> {
    let fake_args = idm::FakeArgs::builder()
        .scan("hci1|00:11:22|Speaker|-65;hci0|AA:BB:CC|IDM-Clock|-43")?
        .build();
    let client = idm::fake_hardware_client(fake_args);

    let session = client.connect_first_device("IDM-").await?;
    let report = session.inspect_report();

    assert_eq!(
        true,
        report.session_metadata().required_endpoints_verified()
    );
    assert_eq!(
        Some(idm::GattProfile::FaFa02),
        report.session_metadata().gatt_profile()
    );
    assert_eq!(
        Some(509),
        report.session_metadata().write_without_response_limit()
    );
    assert_eq!(
        Some("0000fa02-0000-1000-8000-00805f9b34fb"),
        report
            .session_metadata()
            .resolved_endpoint_uuid(idm::EndpointId::WriteCharacteristic)
    );
    assert_eq!(
        Some("0000fa03-0000-1000-8000-00805f9b34fb"),
        report
            .session_metadata()
            .resolved_endpoint_uuid(idm::EndpointId::ReadNotifyCharacteristic)
    );
    assert_eq!(
        None,
        report
            .session_metadata()
            .device_profile()
            .panel_dimensions()
    );
    assert_eq!(None, report.session_metadata().device_profile().led_type());
    assert_eq!(
        509,
        report
            .session_metadata()
            .device_profile()
            .write_without_response_fallback()
    );
    assert_eq!(
        true,
        report
            .endpoint_presence()
            .is_present(idm::EndpointId::ControlService)
    );
    assert_eq!(
        true,
        report
            .endpoint_presence()
            .is_present(idm::EndpointId::WriteCharacteristic)
    );
    assert_eq!(
        true,
        report
            .endpoint_presence()
            .is_present(idm::EndpointId::ReadNotifyCharacteristic)
    );

    session.close().await?;

    Ok(())
}

#[tokio::test]
async fn fake_session_notification_stream_emits_typed_items() -> anyhow::Result<()> {
    let fake_args = idm::FakeArgs::builder()
        .scan("hci0|AA:BB:CC|IDM-Clock|-43")?
        .listen(idm::ListenFixture::TextTransferHappyPath)
        .build();
    let client = idm::fake_hardware_client(fake_args);

    let session = client.connect_first_device("IDM-").await?;
    let mut stream = session
        .notification_stream(
            idm::EndpointId::ReadNotifyCharacteristic,
            Some(2),
            CancellationToken::new(),
        )
        .await?;

    let first = stream
        .next()
        .await
        .expect("stream should emit first item")?;
    let second = stream
        .next()
        .await
        .expect("stream should emit second item")?;
    let ended = stream.next().await;

    assert_eq!(
        idm::NotificationMessage {
            index: 1,
            event: Ok(idm::NotifyEvent::NextPackage(idm::TransferFamily::Text)),
        },
        first
    );
    assert_eq!(
        idm::NotificationMessage {
            index: 2,
            event: Ok(idm::NotifyEvent::Finished(idm::TransferFamily::Text)),
        },
        second
    );
    assert!(ended.is_none());
    let summary: idm::NotificationRunSummary = stream.try_into()?;
    assert_eq!(2, summary.received_notifications());
    assert_matches!(
        summary.stop_reason(),
        &idm::ListenStopReason::ReachedLimit(2)
    );

    session.close().await?;
    Ok(())
}

#[tokio::test]
async fn fake_session_notification_stream_into_summary_requires_completion() -> anyhow::Result<()> {
    let fake_args = idm::FakeArgs::builder()
        .scan("hci0|AA:BB:CC|IDM-Clock|-43")?
        .listen(idm::ListenFixture::TextTransferHappyPath)
        .build();
    let client = idm::fake_hardware_client(fake_args);

    let session = client.connect_first_device("IDM-").await?;
    let stream = session
        .notification_stream(
            idm::EndpointId::ReadNotifyCharacteristic,
            Some(2),
            CancellationToken::new(),
        )
        .await?;
    let result: Result<idm::NotificationRunSummary, idm::InteractionError> = stream.try_into();
    let error = result.expect_err("summary conversion should fail before stream completion");
    assert_matches!(error, idm::InteractionError::NotificationStreamIncomplete);

    session.close().await?;
    Ok(())
}

#[tokio::test]
async fn fake_session_notification_stream_zero_limit_yields_nothing() -> anyhow::Result<()> {
    let fake_args = idm::FakeArgs::builder()
        .scan("hci0|AA:BB:CC|IDM-Clock|-43")?
        .listen(idm::ListenFixture::TextTransferHappyPath)
        .build();
    let client = idm::fake_hardware_client(fake_args);

    let session = client.connect_first_device("IDM-").await?;
    let mut stream = session
        .notification_stream(
            idm::EndpointId::ReadNotifyCharacteristic,
            Some(0),
            CancellationToken::new(),
        )
        .await?;

    let ended = stream.next().await;
    assert!(ended.is_none(), "stream with limit 0 should emit nothing");

    let summary: idm::NotificationRunSummary = stream.try_into()?;
    assert_eq!(0, summary.received_notifications());
    assert_matches!(
        summary.stop_reason(),
        &idm::ListenStopReason::ReachedLimit(0)
    );

    session.close().await?;
    Ok(())
}

#[tokio::test]
async fn fake_session_notification_stream_cancel_produces_interrupted() -> anyhow::Result<()> {
    let fake_args = idm::FakeArgs::builder()
        .scan("hci0|AA:BB:CC|IDM-Clock|-43")?
        .listen(idm::ListenFixture::TextTransferHappyPath)
        .build();
    let client = idm::fake_hardware_client(fake_args);

    let session = client.connect_first_device("IDM-").await?;
    let cancel = CancellationToken::new();

    let mut stream = session
        .notification_stream(
            idm::EndpointId::ReadNotifyCharacteristic,
            None,
            cancel.clone(),
        )
        .await?;

    let first = stream
        .next()
        .await
        .expect("stream should emit first item")?;
    assert_eq!(1, first.index);

    cancel.cancel();

    let ended = stream.next().await;
    assert!(ended.is_none());

    let summary: idm::NotificationRunSummary = stream.try_into()?;
    assert_eq!(1, summary.received_notifications());
    assert_matches!(summary.stop_reason(), &idm::ListenStopReason::Interrupted);

    session.close().await?;
    Ok(())
}
