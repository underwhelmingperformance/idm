use pretty_assertions::assert_eq;
use rstest::rstest;

fn dimensions(width: u16, height: u16) -> idm::PanelDimensions {
    idm::PanelDimensions::new(width, height).expect("test panel dimensions should be valid")
}

#[rstest]
#[case(
    &[0x0F, 0xFF, 0x54, 0x52, 0x00, 0x70, 0x04, 0x01, 0x02, 0x00, 0x01, 0x05, 0x20, 0x00, 0x20, 0x00],
    Some(4)
)]
#[case(&[0x54, 0x52, 0x00, 0x71, 0x03, 0x01, 0x02, 0x00, 0x01, 0x04, 0x20, 0x00, 0x30, 0x00], Some(3))]
#[case(&[0x54, 0x52, 0x00, 0x70, 0x04, 0x05, 0x03, 0x00, 0x08, 0x01], Some(4))]
#[case(&[0x02, 0x01, 0x06], None)]
fn scan_identity_parsing_handles_tlv_and_payload_inputs(
    #[case] raw_scan_data: &[u8],
    #[case] expected_shape: Option<i8>,
) {
    let identity = idm::ScanModelHandler::parse_identity(raw_scan_data);
    assert_eq!(expected_shape, identity.map(|value| value.shape));
}

#[tokio::test]
async fn fake_session_profile_uses_scan_model_payload_when_available() -> anyhow::Result<()> {
    let fake_args = idm::FakeArgs::builder()
        .scan_fixture("hci0|AA:BB:CC|IDM-Clock|-43|5452007004010200010520002000")?
        .build();
    let client = idm::fake_hardware_client(fake_args);

    let session = client.connect_first_device("IDM-").await?;
    let profile = session.device_profile();
    assert_eq!(Some(dimensions(64, 64)), profile.panel_dimensions());
    assert_eq!(idm::ImageUploadMode::RawRgb, profile.image_upload_mode());
    assert_eq!(Some(4), profile.led_type());
    assert_eq!(Some(idm::TextPath::Path6464), profile.text_path());
    assert_eq!(None, profile.joint_mode());
    session.close().await?;

    Ok(())
}

#[tokio::test]
async fn ambiguous_shape_requires_resolution_when_no_led_type_is_available() -> anyhow::Result<()> {
    let fake_args = idm::FakeArgs::builder()
        .scan_fixture("hci0|AAMBIG:01|IDM-1+3|-43|5452007081010200010720002000")?
        .initial_read("0500010001")?
        .build();
    let client = idm::fake_hardware_client(fake_args);

    let result = client.connect_first_device("IDM-").await;
    match result {
        Err(idm::InteractionError::AmbiguousShapeSelectionRequired { shape: -127, .. }) => {}
        Err(other) => panic!("expected ambiguous-shape resolution error, got {other}"),
        Ok(_session) => panic!("expected ambiguous-shape resolution failure"),
    }

    Ok(())
}

#[tokio::test]
async fn ambiguous_shape_resolves_when_led_info_response_is_available() -> anyhow::Result<()> {
    let fake_args = idm::FakeArgs::builder()
        .scan_fixture("hci0|AAMBIG:02|IDM-1+3|-43|5452007081010200010720002000")?
        .initial_read("090001800100000200")?
        .build();
    let client = idm::fake_hardware_client(fake_args);

    let session = client.connect_first_device("IDM-").await?;
    let profile = session.device_profile();
    assert_eq!(Some(dimensions(8, 32)), profile.panel_dimensions());
    assert_eq!(Some(2), profile.led_type());
    assert_eq!(Some(idm::TextPath::Path832), profile.text_path());
    assert_eq!(Some(2), profile.joint_mode());
    session.close().await?;

    Ok(())
}

#[tokio::test]
async fn cid_pid_capability_fallback_resolves_profile_when_shape_is_unknown() -> anyhow::Result<()>
{
    let fake_args = idm::FakeArgs::builder()
        .scan_fixture("hci0|AA64:01|IDM-Unknown|-43|545200702A010200010520002000")?
        .build();
    let client = idm::fake_hardware_client(fake_args);

    let session = client.connect_first_device("IDM-").await?;
    let profile = session.device_profile();
    assert_eq!(Some(dimensions(64, 64)), profile.panel_dimensions());
    assert_eq!(Some(4), profile.led_type());
    assert_eq!(Some(idm::TextPath::Path6464), profile.text_path());
    assert_eq!(None, profile.joint_mode());
    session.close().await?;

    Ok(())
}

#[tokio::test]
async fn ambiguous_cid_pid_family_requires_resolution_when_shape_is_unknown() -> anyhow::Result<()>
{
    let fake_args = idm::FakeArgs::builder()
        .scan_fixture("hci0|AAAMB:01|IDM-Unknown|-43|545200702A010200010120002000")?
        .build();
    let client = idm::fake_hardware_client(fake_args);

    let result = client.connect_first_device("IDM-").await;
    match result {
        Err(idm::InteractionError::AmbiguousShapeSelectionRequired { shape: 42, .. }) => {}
        Err(other) => panic!("expected ambiguous-shape resolution error, got {other}"),
        Ok(_session) => panic!("expected ambiguous-shape resolution failure"),
    }

    Ok(())
}
