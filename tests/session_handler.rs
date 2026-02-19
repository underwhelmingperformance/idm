use assert_matches::assert_matches;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn fake_session_connect_populates_report_metadata() -> anyhow::Result<()> {
    let fake_args = idm::FakeArgs::builder()
        .scan_fixture("hci1|00:11:22|Speaker|-65;hci0|AA:BB:CC|IDM-Clock|-43")?
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
async fn fake_session_run_notifications_respects_limit() -> anyhow::Result<()> {
    let fake_args = idm::FakeArgs::builder()
        .scan_fixture("hci0|AA:BB:CC|IDM-Clock|-43")?
        .notifications("0500010001,0500010003,AA55")?
        .build();
    let client = idm::fake_hardware_client(fake_args);

    let session = client.connect_first_device("IDM-").await?;
    session
        .subscribe_endpoint(idm::EndpointId::ReadNotifyCharacteristic)
        .await?;

    let mut received = Vec::new();
    let run_summary = session
        .run_notifications(
            idm::EndpointId::ReadNotifyCharacteristic,
            Some(2),
            |index, event| {
                received.push((index, event));
            },
        )
        .await?;

    assert_eq!(2, run_summary.received_notifications());
    assert_matches!(
        run_summary.stop_reason(),
        &idm::ListenStopReason::ReachedLimit(2)
    );
    assert_eq!(
        vec![
            (
                1,
                Ok(idm::NotifyEvent::NextPackage(idm::TransferFamily::Gif))
            ),
            (2, Ok(idm::NotifyEvent::Finished(idm::TransferFamily::Gif))),
        ],
        received
    );

    session
        .unsubscribe_endpoint(idm::EndpointId::ReadNotifyCharacteristic)
        .await?;
    session.close().await?;

    Ok(())
}

#[tokio::test]
async fn fake_session_run_listen_returns_summary() -> anyhow::Result<()> {
    let fake_args = idm::FakeArgs::builder()
        .scan_fixture("hci0|AA:BB:CC|IDM-Clock|-43")?
        .initial_read("DEADBEEF")?
        .notifications("AA55,BB66")?
        .build();
    let client = idm::fake_hardware_client(fake_args);

    let session = client.connect_first_device("IDM-").await?;
    let mut received = Vec::new();

    let summary = session
        .run_listen(Some(1), |index, event| {
            received.push((index, event));
        })
        .await?;

    assert_eq!(Some(&[0xDE, 0xAD, 0xBE, 0xEF][..]), summary.initial_read());
    assert_eq!(1, summary.received_notifications());
    assert_matches!(
        summary.stop_reason(),
        &idm::ListenStopReason::ReachedLimit(1)
    );
    assert_eq!(
        vec![(1, Ok(idm::NotifyEvent::Unknown(vec![0xAA, 0x55])))],
        received
    );

    Ok(())
}
