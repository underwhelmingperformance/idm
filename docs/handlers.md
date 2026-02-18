# iDotMatrix Handler Plan

[`protocol.md`](protocol.md) is the normative wire-format and protocol
reference. This document is project-focused and defines which handlers we should
implement, in priority order, with API shape and behaviour expectations.

## Session Handler

Status: `DONE`  
Priority: `P0`

Protocol references:

- [GATT profiles and endpoint resolution](protocol.md#gatt-profiles-and-endpoint-resolution)
- [Profile selection in practice](protocol.md#profile-selection-in-practice)
- [Connection semantics and MTU](protocol.md#connection-semantics-and-mtu)

Behaviour:

- Discover devices and connect.
- Verify required service/characteristic profile.
- Expose negotiated write size and connection metadata.
- Keep transport concerns here; command handlers should not perform discovery.
- Support profile selection for read/notify UUID strategy.

Rust API:

```rust
pub struct SessionHandler<T: BleTransport> {
    transport: T,
}

pub struct SessionProfile {
    pub service_uuid: Uuid,
    pub write_uuid: Uuid,
    pub read_or_notify_uuid: Uuid,
    pub ota_service_uuid: Option<Uuid>,
    pub ota_write_uuid: Option<Uuid>,
    pub ota_notify_uuid: Option<Uuid>,
}

impl<T: BleTransport> SessionHandler<T> {
    pub async fn connect_first(
        &mut self,
        profile: SessionProfile,
    ) -> Result<DeviceSession, InteractionError>;
}
```

## Scan + Model Identification Handler

Status: `DONE`  
Priority: `P0`  
Comment: Manufacturer payload parsing and model resolution are implemented and
wired into discovery. Ambiguous-shape devices now resolve via persisted
per-device LED-type overrides and connect-time LED-info query.

Protocol references:

- [Discovery and model identification](protocol.md#discovery-and-model-identification)
- [Scan acceptance algorithm](protocol.md#scan-acceptance-algorithm)
- [Manufacturer payload layout](protocol.md#manufacturer-payload-layout)
- [Shape and LED type resolution](protocol.md#shape-and-led-type-resolution)

Behaviour:

- Parse BLE advertisement records and validate vendor signature in AD type
  `0xFF`.
- Extract shape, reverse flag, CID, PID, group/device id, and lamp-count fields.
- Resolve model capability profile (panel size, LED type, ambiguity flags).
- Support per-device persisted choice for ambiguous shapes (`0x81/0x82/0x83`).
- Expose resolved capability data to transfer/control handlers.

Rust API:

```rust
pub struct ScanIdentity {
    pub cid: u8,
    pub pid: u8,
    pub shape: i8,
    pub reverse: bool,
    pub group_id: u8,
    pub device_id: u8,
    pub lamp_count: u16,
    pub lamp_num: u16,
}

pub enum AmbiguousShape {
    Shape81,
    Shape82,
    Shape83,
}

pub struct ModelProfile {
    pub led_type: Option<u8>,
    pub panel_size: Option<(u16, u16)>,
    pub ambiguous_shape: Option<AmbiguousShape>,
}

pub struct ScanModelHandler;

impl ScanModelHandler {
    pub fn parse_identity(scan_data: &[u8]) -> Option<ScanIdentity>;
    pub fn resolve_model(identity: &ScanIdentity) -> ModelProfile;
}
```

## Device Profile Resolver Handler

Status: `DONE`  
Priority: `P0`  
Comment: Typed routing resolution is now complete: scan identity + optional LED
info query are merged into canonical LED type/panel/text path/joint mode,
unresolved ambiguity is surfaced as an explicit error, and resolved routing is
surfaced in session metadata and consumed by text upload path validation.

Protocol references:

- [Shape and LED type resolution](protocol.md#shape-and-led-type-resolution)
- [Joint mode mapping for ambiguous layouts](protocol.md#joint-mode-mapping-for-ambiguous-layouts)
- [Canonical model-resolution algorithm](protocol.md#canonical-model-resolution-algorithm)
- [Command routing rules](protocol.md#command-routing-rules)

Behaviour:

- Resolve profile from parsed scan identity plus optional `Get LED type`
  response.
- Emit typed routing decisions for text path (`832/1616/3232/6464/1664`).
- Emit canonical joint mode for ambiguous shapes.
- Keep unknown/ambiguous profile states explicit (no silent fallback to guessed
  panel size).
- Feed this profile into all high-level command handlers.

Rust API:

```rust
pub struct DeviceRoutingProfile {
    pub led_type: Option<u8>,
    pub panel_size: Option<(u16, u16)>,
    pub text_path: Option<TextPath>,
    pub joint_mode: Option<u8>,
}

pub enum TextPath {
    Path832,
    Path1616,
    Path3232,
    Path6464,
    Path1664,
}

pub struct DeviceProfileResolver;

impl DeviceProfileResolver {
    pub fn resolve(identity: &ScanIdentity, led_info: Option<LedInfoResponse>) -> DeviceRoutingProfile;
}
```

## Notification Handler

Status: `DONE`  
Priority: `P0`

Protocol references:

- [Notify responses and flow control](protocol.md#notify-responses-and-flow-control)
- [Device-info query response](protocol.md#device-info-query-response)

Behaviour:

- Subscribe to active notify/read characteristic for the selected profile.
- Decode family-specific statuses (text/gif/image/diy/timer/schedule/OTA).
- Decode state-query responses:
  - LED info response (`0x01/0x80`, including switch/status byte, screen type,
    password flag, and MCU version).
  - Screen-light read response (`0x0F/0x80`).
- Emit typed events for `next_package`, `finish`, and family-specific errors.
- Preserve unknown payloads for diagnostics.

Rust API:

```rust
pub enum TransferFamily {
    Text,
    Gif,
    Image,
    Diy,
    Timer,
    Ota,
}

pub enum NotifyEvent {
    NextPackage(TransferFamily),
    Finished(TransferFamily),
    Error(TransferFamily, u8),
    ScheduleSetup(u8),
    ScheduleMasterSwitch(u8),
    LedInfo(LedInfoResponse),
    ScreenLightTimeout(u8),
    Unknown(Vec<u8>),
}

pub struct NotificationHandler;

impl NotificationHandler {
    pub fn decode(payload: &[u8]) -> Result<NotifyEvent, NotificationDecodeError>;
}
```

## Frame Codec Handler

Status: `DONE`  
Priority: `P0`

Protocol references:

- [Frame families](protocol.md#frame-families)
- [Serialisation rules](protocol.md#serialisation-rules)

Behaviour:

- Encode/decode short control frames.
- Encode/decode 16-byte media headers.
- Encode/decode DIY 9-byte chunk prefixes.
- Encode/decode OTA 13-byte chunk headers.
- Keep field order explicit and per-family; do not assume one universal
  endianness helper behaviour.

Rust API:

```rust
pub struct FrameCodec;

impl FrameCodec {
    pub fn encode_short(command_id: u8, command_ns: u8, payload: &[u8]) -> Result<Vec<u8>, FrameCodecError>;
    pub fn decode_short(frame: &[u8]) -> Result<ShortFrame<'_>, FrameCodecError>;
    pub fn encode_text_header(fields: TextHeaderFields) -> [u8; 16];
    pub fn encode_gif_header(fields: GifHeaderFields) -> [u8; 16];
    pub fn encode_diy_prefix(fields: DiyPrefixFields) -> [u8; 9];
    pub fn encode_ota_chunk_header(fields: OtaChunkHeaderFields) -> [u8; 13];
}
```

## Power Handler

Status: `DONE`  
Priority: `P0`

Protocol reference: [Device/common control](protocol.md#devicecommon-control)
(Switchplate)

Behaviour:

- Implement screen on/off command pair where supported.
- Ensure idempotent caller-facing behaviour.
- CLI wired: `idm control power <off|on>`.

## Brightness Handler

Status: `DONE`  
Priority: `P0`

Protocol reference: [Device/common control](protocol.md#devicecommon-control)
(Brightness)

Behaviour:

- Send brightness command with validated range input.
- Reject out-of-range values before encoding.
- CLI wired: `idm control brightness <0..100>`.

## Time Sync Handler

Status: `DONE`  
Priority: `P0`

Protocol reference: [Device/common control](protocol.md#devicecommon-control)
(Sync time)

Behaviour:

- Convert timestamp to protocol fields (`yy mm dd dow HH MM SS`).
- Use Monday=1..Sunday=7 for day-of-week encoding.
- Send one atomic synchronisation frame.
- CLI wired: `idm control sync-time [--unix <timestamp>]`.

## Fullscreen Colour Handler

Status: `DONE`  
Priority: `P0`

Protocol reference: [Device/common control](protocol.md#devicecommon-control)
(Full-screen colour)

Behaviour:

- Fill display with one RGB colour via fullscreen command.
- Use typed colour parameters.
- CLI wired: `idm control colour <r> <g> <b>`.

## Text Upload Handler

Status: `DONE`  
Priority: `P0`

Protocol references:

- [Text upload](protocol.md#text-upload)
- [Command routing rules](protocol.md#command-routing-rules) (text-path rules)
- [Transfer family ACK patterns](protocol.md#transfer-family-ack-patterns)
  (Text)

Behaviour:

- Build text payload from metadata + glyph stream according to selected
  resolution/profile.
- Compute CRC32 over logical text payload.
- Chunk at protocol size and then transport size.
- Use notification-driven pacing with fallback timeout policy.
- CLI wired: `idm control text <text>`.

## GIF Upload Handler

Status: `TODO`  
Priority: `P0`

Protocol references:

- [GIF upload](protocol.md#gif-upload)
- [Shared media tail byte pattern](protocol.md#shared-media-tail-byte-pattern-gif-image-text)
- [Transfer family ACK patterns](protocol.md#transfer-family-ack-patterns) (GIF)

Behaviour:

- Chunk raw GIF bytes at `4096` protocol chunk size.
- Emit 16-byte GIF headers with proper flags/CRC/tail profile.
- Use notification-driven flow control.

## Image Upload Handler (Non-DIY)

Status: `TODO`  
Priority: `P1`

Protocol references:

- [Image upload (non-DIY)](protocol.md#image-upload-non-diy)
- [Shared media tail byte pattern](protocol.md#shared-media-tail-byte-pattern-gif-image-text)
- [Transfer family ACK patterns](protocol.md#transfer-family-ack-patterns)
  (Image)

Behaviour:

- Support image family (`0x02`) 16-byte header upload flow.
- Align tail/profile bytes to target firmware behaviour.
- Reuse chunker and flow-control primitives from GIF handler.

## DIY Upload Handler (Raw RGB)

Status: `TODO`  
Priority: `P1`

Protocol references:

- [DIY raw RGB upload](protocol.md#diy-raw-rgb-upload)
- [DIY raw-image frame](protocol.md#diy-raw-image-frame-9-byte-header)
- [Transfer family ACK patterns](protocol.md#transfer-family-ack-patterns) (DIY)

Behaviour:

- Switch to DIY mode before transfer.
- Upload RGB payload with 9-byte DIY per-chunk prefix.
- Support mode reset/exit behaviour after completion.

## Clock Style Handler

Status: `TODO`  
Priority: `P1`

Protocol reference: [Device/common control](protocol.md#devicecommon-control)
(Clock mode/style)

Behaviour:

- Encode clock style flags (`style`, `show_date`, `is_24h`) and colour.
- Keep style as typed enum.

## Countdown Handler

Status: `TODO`  
Priority: `P1`

Protocol reference: [Device/common control](protocol.md#devicecommon-control)
(Countdown)

Behaviour:

- Support start/pause/reset style countdown commands.
- Validate minute/second ranges.

## Chronograph Handler

Status: `TODO`  
Priority: `P1`

Protocol reference: [Device/common control](protocol.md#devicecommon-control)
(Chronograph)

Behaviour:

- Map semantic operations to protocol modes (`reset/start/pause/continue`).

## Scoreboard Handler

Status: `TODO`  
Priority: `P1`

Protocol reference: [Device/common control](protocol.md#devicecommon-control)
(Scoreboard)

Behaviour:

- Encode two score words with explicit byte ordering policy.
- Clamp or reject out-of-range values by documented policy.

## Device Info Handler

Status: `TODO`  
Priority: `P1`  
Comment: Expanded scope on `2026-02-16`. LED-info response parsing is used both
from explicit query semantics and from the time-sync callback path; current
implementation assumptions around a single query flow are incomplete.

Protocol reference:
[Device-info query response](protocol.md#device-info-query-response)

Behaviour:

- Support LED-info read via protocol response shape
  `len >= 9, b[2]=0x01, b[3]=0x80` and parse:
  - MCU version major/minor
  - switch/status byte
  - screen type
  - password flag
- Optionally issue dedicated query frame (`04 00 01 80`) where supported.
- Surface capabilities needed by higher-level handlers.

## Experimental: `getLedType` Probe

Status: `EXPERIMENTAL`  
Priority: `P1`  
Comment: Added on `2026-02-16`. A dedicated query frame `04 00 01 80` is defined
but call-sites are not explicit in normal flows. We should probe this safely and
capture per-device behaviour.

Protocol reference:
[Device-info query response](protocol.md#device-info-query-response)

Behaviour:

- Send raw query frame `04 00 01 80` after connection/notify setup.
- Wait for response matching `len >= 9`, `b[2] == 0x01`, `b[3] == 0x80`.
- Parse and log:
  - MCU version major/minor (`b[4]`, `b[5]`)
  - switch/status byte (`b[6]`)
  - screen type (`b[7]`)
  - password flag (`b[8]`)
- Apply timeout and no-response handling; do not block normal command flows.
- Record outcome by resolved profile/firmware so we can decide if this should be
  promoted into the core `Device Info Handler`.

## Screen Light Timeout Handler

Status: `TODO`  
Priority: `P1`  
Comment: Expanded scope on `2026-02-16`. Query/readback (`05 00 0F 80 FF` ->
`05 00 0F 80 <value>`) is confirmed; implementation must expose typed readback,
not set-only writes.

Protocol reference: [Device/common control](protocol.md#devicecommon-control)
(Screen-light timeout set/read)

Behaviour:

- Implement set/read screen-light timeout commands.
- Parse readback response and expose typed duration.

## Readback Capability Handler

Status: `TODO`  
Priority: `P1`  
Comment: New on `2026-02-16`. Only limited readback capability exists (LED info,
screen-light timeout, OTA descriptor string). No reverse
framebuffer/current-image download command path was found.

Protocol references:

- [Device-info query response](protocol.md#device-info-query-response)
- [Compatibility notes](protocol.md#compatibility-notes)

Behaviour:

- Provide one place that defines supported read operations and unsupported ones.
- Support:
  - LED info/state read.
  - Screen-light timeout read.
  - Optional descriptor-string read for firmware/OTA gating.
- Explicitly return `unsupported` for:
  - Download currently displayed image/framebuffer.
  - Pulling current material payload bytes from device.

## Password Handler

Status: `TODO`  
Priority: `P2`

Protocol reference: [Device/common control](protocol.md#devicecommon-control)
(Set/Verify password)

Behaviour:

- Implement set password and verify password command families.
- Keep password representation typed and validated.

## Joint/Mic/Rhythm Handler

Status: `TODO`  
Priority: `P2`

Protocol references:

- [Joint mode mapping for ambiguous layouts](protocol.md#joint-mode-mapping-for-ambiguous-layouts)
- [Music/rhythm control](protocol.md#musicrhythm-control)

Behaviour:

- Implement joint mode, mic type, and rhythm control commands.
- Keep unknown sub-modes explicit as opaque values where semantics are unclear.

## Timer Transfer Handler

Status: `TODO`  
Priority: `P1`

Protocol references:

- [Timer transfer protocols](protocol.md#timer-transfer-protocols)
- [Timer transfer frame](protocol.md#timer-transfer-frame-24-byte-header)
- [Transfer family ACK patterns](protocol.md#transfer-family-ack-patterns)
  (Timer)

Behaviour:

- Encode timer transfer headers (24-byte family).
- Support image/text timer payload modes.
- Decode timer-specific ACK/save/fail notify statuses.

## Schedule Transfer Handler

Status: `TODO`  
Priority: `P1`

Protocol references:

- [Schedule transfer protocols](protocol.md#schedule-transfer-protocols)
- [Schedule transfer frame](protocol.md#schedule-transfer-frame-23-byte-header)
- [Schedule control responses](protocol.md#schedule-control-responses)

Behaviour:

- Encode schedule transfer headers (23-byte family).
- Support gif/image/text schedule theme transfers.
- Sequence setup/master-switch and queued resource sends.
- Decode schedule setup/master-switch responses.

## OTA Handler

Status: `TODO`  
Priority: `P1`

Protocol references:

- [OTA transfer protocol](protocol.md#ota-transfer-protocol)
- [OTA data frame](protocol.md#ota-data-frame)
- [OTA setup command](protocol.md#ota-setup-command)
- [Transfer family ACK patterns](protocol.md#transfer-family-ack-patterns) (OTA)

Behaviour:

- Implement OTA step-1 command and validation.
- Encode OTA chunk headers and send chunked binary.
- Track OTA notify statuses and fail conditions.

## Display Orientation Handler

Status: `TODO`  
Priority: `P2`

Protocol reference: [Device/common control](protocol.md#devicecommon-control)
(Rotate 180)

Behaviour:

- Manage rotate control as typed operation.
- Keep each control explicit; avoid ambiguous boolean APIs.

## Graffiti Handler

Status: `TODO`  
Priority: `P2`

Protocol references:

- [DIY raw RGB upload](protocol.md#diy-raw-rgb-upload)
- [Device/common control](protocol.md#devicecommon-control) (Enter/exit DIY
  mode)

Behaviour:

- Support per-pixel DIY drawing command path.
- Validate coordinates against active panel dimensions.

## Cross-cutting requirements

- All transfer handlers SHOULD share a common chunking/pacing engine.
- Timeout policy MUST be configurable per transfer family.
- Handler APIs SHOULD return structured receipts with status family and final
  response payload.
- Unknown response payloads MUST be preserved for diagnostics.
