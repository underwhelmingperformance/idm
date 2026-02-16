# iDotMatrix BLE Protocol Research Notes

This document records the evidence trail behind `docs/protocol.md`.

It is a working reverse-engineered record, not vendor documentation.

## Research status

- Updated: `2026-02-16`
- Primary profile analysed: decompiled Android vendor app build
  `iDotMatrix_2026_01_07_16_53-v2.0.0_googleRelease`
- Scope: BLE transport, discovery/model identification, command frames, bulk
  transfer families, and notify semantics.

## Primary decompiled sources

- `apk/decompiled/sources/com/heaton/baselib/ble/BleManager.java`
- `apk/decompiled/sources/com/heaton/baselib/ble/BleConfig.java`
- `apk/decompiled/sources/cn/com/heaton/blelibrary/ble/Ble.java`
- `apk/decompiled/sources/cn/com/heaton/blelibrary/ble/model/BleDevice.java`
- `apk/decompiled/sources/com/tech/idotmatrix/AppData.java`
- `apk/decompiled/sources/com/tech/idotmatrix/ble/BleProtocol.java`
- `apk/decompiled/sources/com/tech/idotmatrix/ble/BleProtocolN.java`
- `apk/decompiled/sources/com/tech/idotmatrix/ble/send/BaseSend.java`
- `apk/decompiled/sources/com/tech/idotmatrix/core/data/TextAgreement.java`
- `apk/decompiled/sources/com/tech/idotmatrix/core/data/GifAgreement.java`
- `apk/decompiled/sources/com/tech/idotmatrix/core/data/ImageAgreement.java`
- `apk/decompiled/sources/com/tech/idotmatrix/core/data/TimerAgreement.java`
- `apk/decompiled/sources/com/tech/idotmatrix/core/data/ScheduleAgreement.java`
- `apk/decompiled/sources/com/tech/idotmatrix/core/data/OTAAgreement.java`
- `apk/decompiled/sources/com/tech/idotmatrix/ota/OtaUpData.java`
- `apk/decompiled/sources/com/tech/idotmatrix/p009ui/connect/DeviceConnectActivity.java`
- `apk/decompiled/sources/com/tech/idotmatrix/p009ui/connect/DeviceTypeSelectDialog.java`
- `apk/decompiled/sources/com/tech/idotmatrix/p009ui/adapter/DeviceAdapter.java`
- `apk/decompiled/sources/com/tech/idotmatrix/p009ui/pattern/main/MainActivity.java`
- `apk/decompiled/sources/com/tech/idotmatrix/p009ui/find/FindFragment.java`

## Confidence labels

- `Confirmed`: directly present in decompiled send/parse codepaths.
- `Inferred`: highly likely from adjacent code, naming, or flag handling.
- `Unknown`: bytes or behaviour observed, semantics still unclear.

## Major corrections from earlier docs

## 1) Discovery by local name is insufficient

`BleConfig.matchProduct` and the scan listeners in both `DeviceConnectActivity`
and `MainActivity` filter by manufacturer AD record (`0xFF`) with `TR 00 70` or
`TR 00 71` signatures.

Name-prefix-only scanning is not equivalent to vendor behaviour.

`matchProduct` also enforces an AD-length guard (`len > 31 => false`) while
walking TLV records. This guard should be treated as vendor parser behaviour,
not generic BLE validation.

## 2) Manufacturer payload offsets are implementation-critical

`BleConfig` field extractors are all offset-based and consistent:

- shape/device type: `[5]`
- group id: `[6]`
- device id: `[7]`
- reverse: `[8]`
- CID: `[9]`
- PID: `[10]`
- version marker: `[11]`
- lamp count: `[12,11]`
- lamp num: `[14,13]`

Each extractor validates only the offset it needs, so shape/CID/PID can still
be read when trailing lamp bytes are missing in truncated manufacturer values.

## 3) Blue filter semantics are exclusionary

`matchBlueFiler(list, adv)` checks `"000{cid}0{pid}"` membership. Caller logic
rejects the scan result when it matches, so this acts as a blocklist in observed
paths.

## 4) UUID profile evidence is split across two layers

`Ble.Options` defaults:

- `fee9` service
- `d44...600/601/602` characteristics

`BleManager.init()` runtime override:

- `fa` service
- `fa02` write
- `d44...602` read

`ae00/ae01/ae02` OTA profile is explicit and stable.

This APK contains both profiles; robust clients should negotiate at runtime.

## 5) MTU and write chunking are explicit

`BleManager` requests MTU `512`. Agreement classes then use:

- `509` payload bytes when MTU status is ready
- `18` payload bytes fallback

This appears in text/gif/image/timer/schedule/OTA flows.

## 6) Model identification uses shape + CID/PID + persisted choice

- LED type constants and dimensions are in `AppData`.
- Ambiguous shapes `0x81`, `0x82`, `0x83` trigger type-selection UI.
- Joint command (`0x0C`) is sent after selection.
- `DeviceAdapter.generateBleName` provides a CID/PID map for known families.

## 7) Shape `0x82` handling is incomplete in decompiled UI logic

`DeviceTypeSelectDialog` presents `16x32` vs `8x64` for `0x82`, but mapping to
`curType`/joint command is incomplete in the decompiled branch. Treat this as an
open protocol gap.

`AppData.LedType` does not define a dedicated `8x64` constant, which reinforces
that `0x82` handling is not resolved cleanly in this app version.

## 8) Joint replay path is inconsistent across app flows

`DeviceTypeSelectDialog` maps LED type to joint command values (`1/2/5/6`) when
the user confirms selection. A reconnect path in `MainActivity` replays
`sendJoint(curType)` using raw LED type (`1/2/3/11`), which conflicts with the
dialog mapping.

For our implementation, use canonical joint mapping and do not mirror this
inconsistency.

## 9) Control-command corrections remain valid

- Reset is single frame: `04 00 03 80`.
- Mic type command is `05 00 0B 80 {type}`.
- Countdown and chronograph mode bytes map to `reset/start/pause/continue` in
  app usage.

## 10) Readback response formats are known for key controls

- LED info response guard: `len >= 9`, `b[2]==0x01`, `b[3]==0x80`.
  - Observed parse fields in `MainActivity`:
    - `b[4]` = MCU version major
    - `b[5]` = MCU version minor
    - `b[6]` = switch/status byte used to update UI on/off state
    - `b[7]` = screen type byte
    - `b[8]` = password-flag byte
  - This parse happens in the callback used with
    `BleProtocolN.synchronizedTime(...)`.
- Screen-light read response: `[05,00,0F,80,value]`.
  - Query is sent by `BleProtocolN.readScreenLight()` (`05 00 0F 80 FF`).
  - Response is consumed via `BleDataCallback.onDataReceived(...)` in
    `FindFragment`.
- `getLedType` query frame (`04 00 01 80`) is defined in `BaseSend`, but no
  explicit call site was found in decompiled UI/core flows for this APK build.

## 11) BLE read capability exists but is narrow in app usage

- Library-level support exists for characteristic and descriptor reads:
  - `Ble.read(...)`
  - `Ble.readDes(...)`
  - `ReadRequest.read(...)`
  - `ReadRequest.readDes(...)`
- App-level usage is effectively descriptor-read only:
  - `BleManager` calls `Ble.getInstance().readDes(...)` after MTU setup and
    parses an ASCII firmware/version string for OTA gating.
- No app call site was found for `Ble.read(...)` against
  `uuid_read_ota_cha (..9602)` in normal operation.

## 12) Text transfer behaviour is model-path dependent

Text agreement has separate encoder paths for at least:

- `8x32`
- `16x16` class (`16x16`, `16x32`, `24x48`)
- `32x32`
- `64x64`
- `16x64`

All five paths are now documented in `protocol.md` section 9.1:

- `sendTextTo832` (8x32): 12x12 rasterisation, type tags 0x04/0x00/0x01.
- `sendTextTo1616` (16x16-class): 8x16/16x16, type tags 0x02/0x03.
- `sendTextTo3232` (32x32): font size parameter (16 or 32), type tags
  0x02/0x03/0x05/0x06.
- `sendTextTo6464` (64x64): font size parameter (16, 32, or 64), type tags
  0x02/0x03/0x05/0x06/0x07/0x08.
- `sendTextTo1664` (16x64): fixed 16px, type tags 0x02/0x03.

Routing to these paths is confirmed in multiple UI layers (`TextActivity`,
`NewDeviceMaterialChildFragment`, `MaterialDetailsListActivity`) and follows LED
type dispatch (`1/6/7 -> 1616`, `2 -> 832`, `3 -> 3232`, `4 -> 6464`,
`11 -> 1664`).

## 13) Notification handling is family-specific

The app parses and reacts to ACK/status families for text, GIF, image, DIY,
timer, schedule, and OTA. This is not a write-only protocol profile.

## 14) OTA start ACK accepts two variants

`OtaUpData` accepts either of these step-1 acknowledgements before chunk send:

- `05 00 00 C0 01`
- `05 00 02 C0 01`

## 15) No evidence of reverse framebuffer/material download over BLE

- Decompiled send/parse flows contain many write/ACK families and a small set of
  state reads (LED info, screen-light timeout, OTA descriptor string).
- No command/handler was found that downloads the currently displayed image,
  GIF, or full framebuffer contents back from device to phone.
- No command/handler was found that enumerates "current material slot" by
  pulling frame data from device; material sync paths in UI are push-driven.

## Command families confirmed in decompiled send paths

## Short control families

- Time sync
- Eco mode
- Brightness
- Screen-light timeout set/read
- Rotate
- Time indicator toggle
- Countdown
- Chronograph
- Scoreboard
- Mic type and rhythm commands
- Full-screen colour
- Speed
- Enter/exit DIY
- Clock style/mode
- Switchplate
- Set/verify password
- Joint mode
- Get LED type
- Reset
- Delete material
- Encrypted locate command

## Bulk transfer families

- Text (`family=0x03`, 16-byte per-chunk header)
- GIF (`family=0x01`, 16-byte per-chunk header)
- Image (`family=0x02`, 16-byte per-chunk header)
- DIY raw image (`family=0x00`, 9-byte per-chunk header)
- Timer content transfers (24-byte header)
- Schedule theme transfers (23-byte header)
- OTA content transfers (13-byte header)

## Notify/ACK families observed

- Text: `... 00 03 00 {01|02|03}`
- GIF: `... 00 01 00 {00|01|02|03}`
- Image: `... 00 02 00 {01|02|03}`
- DIY: `... 00 00 00 {00|01|02}`
- Timer: `... 00 00 80 {00|01|03}`
- OTA: `... 00 01 C0 {00|01|03}`
- Schedule setup/master-switch responses:
  - `05 00 05 80 {value}`
  - `05 00 07 80 {value}`

## Device-specific branching audit (confirmed paths)

- **Text upload**: 5 paths routed by LED type (§9.1). 8x32 path has mode
  increment for LED type 2.
- **GIF/Image upload**: Shared tail byte pattern (§9.2.1) — simple vs.
  material/scheduled mode.
- **Timer text**: LED type 2 mode increment (§9.5.1).
- **Schedule text**: LED type 2/11 uses 8x32-class rendering; other types use
  16x16-class. Width selection for non-2/11 path uses `isChinese()` only
  (§9.6.1).
- **Short control commands**: No device-specific branching confirmed in
  BleProtocolN send paths (brightness, countdown, scoreboard, etc. all use fixed
  byte structures).
- **OTA/DIY**: No device-specific branching.

## Resolved gaps (from earlier research rounds)

1. **DeviceMaterialTimeConvert**: Fully resolved. `ConvertTime(timeSign)` maps
   UI selection (0-4) to display duration in seconds (5/10/30/60/300). Packed as
   u16_le into media header bytes [13:14]. Source:
   `DeviceMaterialTimeConvert.java`.

2. **UUID profile selection**: The FA/FA02 override in `BleManager.init()` is
   unconditional — no device-type, CID/PID, or firmware check. No `fa03` UUID
   exists. Notify endpoints are auto-detected by GATT property flags.

3. **Shape 0x82**: Joint mode mapping follows standard rules (type 1 → joint 1,
   type 2 → joint 2). The `8x64` UI option maps to LED type 2 (internally `8x32`
   / `{8, 32}`). The UI resource label `device_type_8x64` disagrees with the
   stored panel size. Unresolved: whether firmware interprets this differently
   on 0x82 hardware.

4. **1+3 and 1+15 families**: These are naming labels for devices with ambiguous
   shape bytes (0x81/0x82/0x83). They do not correspond to specific panel sizes.
   Users must select the actual layout via `DeviceTypeSelectDialog`. Name
   format: `IDM_1+3_{MAC_SUFFIX}` / `IDM_1+15_{MAC_SUFFIX}`.

5. **Complete ACK status map**: All families documented with byte-level
   precision in protocol.md §10. Key finding: DIY uses inverted status semantics
   (0x02 = next, 0x00/0x01 = finish). Timer uses 0x01 for both next and complete
   (context-dependent). Schedule uses a different dispatch pattern (checks [0]
   not [1]).

6. **Font size parameter**: Default is 16. UI offers 16/32 for 32x32 panels,
   16/32/64 for 64x64 panels. Stored in `Material.fontSize`. All three sizes are
   used in practice.

7. **Schedule text isChinese-only**: Confirmed as genuinely narrower than other
   text paths. Likely an app bug — implementations should use the full
   CJK/Japanese/Korean width check.

## Evidence-backed caveats

- The APK contains FEE9/D44 defaults but always overrides to FA/FA02. Whether
  FEE9-only devices exist in the wild is unknown.
- Shape 0x82 `8x64` option stores `8x32` dimensions — firmware behaviour on real
  hardware may differ.
- Schedule text width selection is narrower than other paths (isChinese only).

## Open gaps

1. Whether shape `0x82` firmware reinterprets LED type 2 as `8x64` on that
   hardware (requires real device testing).
2. Whether FEE9/D44 profile devices exist in the wild.
3. Additional ACK status codes that may exist on firmware variants not covered
   by this APK version.

## Relationship to older ecosystem sources

Earlier ecosystem references from Python/Go/Swift/Home Assistant repositories
remain useful for cross-checking. For this repository, decompiled vendor app
behaviour takes precedence when sources conflict.
