# iDotMatrix BLE Protocol Specification (Unofficial)

## Document status

This is an unofficial protocol reference for iDotMatrix devices, derived from
reverse-engineering and decompiled vendor-app sources.

It is intended to be implementation-ready for this repository.

- Version: `0.6.0-draft`
- Date: `2026-02-16`
- Normative language: `MUST`, `SHOULD`, `MAY` as defined by RFC 2119.

## Scope

This specification covers:

- BLE discovery and model identification.
- GATT endpoint/profile resolution.
- Command frame formats and high-level command mapping.
- Large payload transfer procedures (text, GIF, image/DIY, timer, schedule,
  OTA).
- Known acknowledgement behaviour and flow-control signals.
- Explicitly unknown protocol areas.

This specification does not claim to describe every firmware variant.

## Discovery and model identification

### Scan acceptance algorithm

Implementations SHOULD process BLE advertisement records as TLV AD structures,
not local-name prefixes only.

A candidate device is accepted when all of the following are true:

1. At least one AD structure has type `0xFF` (manufacturer-specific data).
2. That manufacturer payload starts with one of these 4-byte signatures:
   - `54 52 00 70` (`TR\x00p`)
   - `54 52 00 71` (`TR\x00q`, OMNILED variant)
3. Optional user filter does not exclude it (see
   [CID/PID filtering](#optional-cidpid-filtering-semantics)).

#### AD-TLV parser constraints

Scan bytes are parsed as repeated AD-TLV records:

- first byte: `len`
- next `len` bytes: one AD record payload

Observed guard behaviour:

- `len <= 0`: parser skips forward.
- `len > 31`: function returns `false` immediately.

Implementations MAY keep this `len <= 31` guard. At minimum, malformed AD
records SHOULD be rejected.

### Manufacturer payload layout

Field offsets are relative to the matched manufacturer AD structure where index
`0` is AD type `0xFF`.

```text
offset  size  field
0       1     ad_type (0xFF)
1..4    4     signature (54 52 00 70 | 54 52 00 71)
5       1     shape/device_type byte (signed in Java)
6       1     group_id
7       1     device_id
8       1     reverse flag (non-zero means reversed)
9       1     cid (vendor id)
10      1     pid (product id)
11      1     version_25 marker
11..12  2     lamp_count_u16_le
13..14  2     lamp_num_u16_le
```

Notes:

- `lamp_count` is read as bytes `[12], [11]` in code, equivalent to
  little-endian decoding of bytes `11..12`.
- `lamp_num` is read as `[14], [13]`, equivalent to little-endian decoding of
  bytes `13..14`.
- Some adapters (notably CoreBluetooth) may expose truncated manufacturer
  payload values where trailing bytes (`version_25`, `lamp_count`, `lamp_num`)
  are absent. Implementations SHOULD still parse partial identity when bytes
  `shape..pid` are present and treat missing lamp fields as unknown.

#### Derived flags from manufacturer data

- `isNewDevice`: `cid >= 1`. Gates availability of newer features.
- `isNewDeviceRhythm`: `cid == 2`. Gates rhythm/music visualisation features.

### Shape and LED type resolution

The `shape` byte (`offset 5`) is used for panel layout decisions.

Known LED types:

- `1` => `16x16`
- `2` => `8x32`
- `3` => `32x32`
- `4` => `64x64`
- `6` => `24x48`
- `7` => `16x32`
- `11` => `16x64`

Ambiguous shape values (signed byte form):

- `-127` (`0x81`): user chooses `16x16` (type `1`) or `8x32` (type `2`)
- `-126` (`0x82`): UI presents `16x32` (LED type `1`) or `8x64` (LED type `2`)
- `-125` (`0x83`): user chooses `32x32` (type `3`) or `16x64` (type `11`)

There is no dedicated `8x64` LED type constant. When the user selects `8x64` for
shape `0x82`, LED type `2` is stored with panel dimensions `{8, 32}`. The UI
labels this as `8x64` but the stored dimensions say `8x32`. Whether the device
firmware reinterprets LED type `2` differently on `0x82` hardware is
unconfirmed. Implementations SHOULD treat this panel size as configurable.

For ambiguous devices, implementations SHOULD persist per-device choice and send
joint mode after connect (see
[Joint mode mapping](#joint-mode-mapping-for-ambiguous-layouts)).

### Joint mode mapping for ambiguous layouts

After type selection, the client sends `05 00 0C 80 {mode}`.

Observed mapping:

- LED type `1` => joint mode `1`
- LED type `2` => joint mode `2`
- LED type `3` => joint mode `5`
- LED type `11` => joint mode `6`

For shape `0x82`, the mapping is consistent with the general rule (LED type `1`
=> joint `1`, LED type `2` => joint `2`), but the underlying panel size
ambiguity for the `8x64` option remains (see
[Shape and LED type resolution](#shape-and-led-type-resolution)).

A known implementation bug sends raw LED type values (`1/2/3/11`) as joint mode
instead of the mapped values (`1/2/5/6`). Implementations SHOULD always use the
canonical mapping above.

### CID/PID model map

Known CID/PID pairs by panel type (format: `CID_PID`):

- `16x16`: `1_3`, `1_19`, `2_3`, `4_3`, `5_1`, `5_2`, `6_1`
- `32x32`: `1_4`, `1_20`, `2_4`, `3_2`, `4_4`
- `64x64`: `1_5`, `4_7`
- `8x32`: `1_6`, `1_25`
- `16x32`: `1_21`
- `24x48`: `1_22`
- `1+3` family: `1_1`, `3_1`, `4_1`, `1_7`, `4_5`
- `1+15` family: `1_2`, `4_2`, `1_8`, `4_6`

The `1+3` and `1+15` families are naming labels for devices that advertise
ambiguous shape bytes (`0x81`/`0x82`/`0x83`). They do not map to specific panel
sizes — the user must select the actual panel layout via the type-selection
dialog (see [Shape and LED type resolution](#shape-and-led-type-resolution)).
The display name format is `IDM_1+3_{MAC_SUFFIX}` or `IDM_1+15_{MAC_SUFFIX}`.

This map is useful for fallback inference and UX labels, but active behaviour
SHOULD still prefer shape + LED-type query/selection.

### Device-info query response

Command:

- `04 00 01 80` (Get LED type)

Response format:

- Guard: `len >= 9`, `b[2] == 0x01`, `b[3] == 0x80`
- `b[4]`: MCU major version
- `b[5]`: MCU minor version
- `b[6]`: status byte (semantics unclear)
- `b[7]`: screen type / LED type byte
- `b[8]`: password flag

### Optional CID/PID filtering semantics

An optional CID/PID exclusion filter checks whether a list contains the literal
string `"000{CID}0{PID}"`. A match causes scan result rejection.

## GATT profiles and endpoint resolution

### Known GATT profiles

Two related endpoint sets exist:

Library defaults (`Ble.Options`):

- Service: `0000fee9-0000-1000-8000-00805f9b34fb`
- Write/read: `d44bc439-abfd-45a2-b575-925416129600`
- Notify: `d44bc439-abfd-45a2-b575-925416129601`
- Read version: `d44bc439-abfd-45a2-b575-925416129602`

Runtime override (always applied at initialisation):

- Service: `000000fa-0000-1000-8000-00805f9b34fb`
- Write: `0000fa02-0000-1000-8000-00805f9b34fb`
- Read UUID configured: `d44bc439-abfd-45a2-b575-925416129602`

OTA channel:

- Service: `0000ae00-0000-1000-8000-00805f9b34fb`
- Write: `0000ae01-0000-1000-8000-00805f9b34fb`
- Notify: `0000ae02-0000-1000-8000-00805f9b34fb`

### Profile selection in practice

The only known client unconditionally uses FA/FA02 regardless of device type,
CID/PID, or firmware version — there is no runtime negotiation or fallback to
FEE9.

For notify, any characteristic with the NOTIFY (`0x10`) or INDICATE (`0x20`)
property flag is subscribed to, regardless of UUID.

OTA endpoints (`ae00/ae01/ae02`) are expected to be present when OTA operations
are initiated.

### Recommended endpoint negotiation for implementations

Implementations SHOULD negotiate endpoints from discovered services to support
both profile variants:

1. Control service/write:
   - Prefer `fa/fa02` if present.
   - Else accept `fee9/d44...600`.
2. Notify/read endpoint:
   - Prefer `d44...602` when present and readable/notifiable.
   - Else `d44...601` when notifiable.
   - Else any notifiable characteristic on the matched service.
   - Else fail with explicit endpoint-missing error.
3. OTA endpoints:
   - Enable only when `ae00/ae01/ae02` all exist.

### Connection semantics and MTU

- No application-layer pairing handshake is required for normal commands.
- Clients SHOULD request MTU `512` after connect.
- Payload chunk size:
  - `509` bytes when MTU negotiation succeeds (threshold: `mtu >= 100`)
  - `18` bytes fallback otherwise

Only one controller should be connected at a time; concurrent connections are
not coordinated.

## Serialisation rules

- Multibyte integers are little-endian on the wire.
- Length fields in frame headers are u16 little-endian.
- Implementations MUST use explicit packing logic and SHOULD verify field order
  by command family.

## Frame families

### Short control frame

Most control commands use this format:

```text
offset  size  field
0..1    2     frame_len_u16_le
2       1     command_id
3       1     command_ns
4..N    *     payload
```

### 16-byte media header frame

Text (`family=0x03`), GIF (`0x01`), and image (`0x02`) upload families use a
16-byte per-chunk header:

```text
offset  size  field
0..1    2     block_len_u16_le (16 + chunk_len)
2       1     family
3       1     fixed_0x00
4       1     chunk_flag (first=0x00, continuation=0x02)
5..8    4     total_payload_len_u32_le
9..12   4     crc32_u32_le
13..15  3     family tail/profile bytes
```

### DIY raw-image frame (9-byte header)

DIY raw image uploads use a 9-byte per-chunk prefix:

```text
offset  size  field
0..1    2     block_len_u16_le (9 + chunk_len)
2       1     fixed_0x00
3       1     fixed_0x00
4       1     chunk_flag (first=0x00, continuation=0x02)
5..8    4     total_payload_len_u32_le
```

### Timer transfer frame (24-byte header)

Timer payload transfer frame:

```text
offset  size  field
0..1    2     block_len_u16_le (24 + chunk_len)
2       1     fixed_0x00
3       1     fixed_0x80
4       1     timer_num
5       1     week
6       1     start_hour
7       1     start_min
8..9    2     duration_seconds_u16_le
10      1     timer_type
11      1     buzzer_enable
12      1     chunk_flag (0x00/0x02)
13..16  4     total_payload_len_u32_le
17..20  4     crc32_u32_le
21      1     fixed_0x00
22      1     fixed_0x00
23      1     tail_selector (timer_num + 20)
```

Timer close frame is a separate 12-byte control payload without transfer fields.

### Schedule transfer frame (23-byte header)

Schedule theme frame:

```text
offset  size  field
0..1    2     block_len_u16_le (23 + chunk_len)
2       1     fixed_0x05
3       1     fixed_0x80
4       1     index
5       1     week_mask
6       1     start_hour
7       1     start_min
8       1     end_hour
9       1     end_min
10      1     theme_type (gif=1, image=2, text=3)
11      1     chunk_flag (0x00/0x02)
12..15  4     total_payload_len_u32_le
16..19  4     crc32_u32_le
20      1     fixed_0x00
21      1     fixed_0x00
22      1     tail_selector (index + 30)
```

### OTA data frame

OTA data chunks use a 13-byte header:

```text
offset  size  field
0..1    2     block_len_u16_le (13 + chunk_len)
2       1     fixed_0x01
3       1     fixed_0xC0
4       1     package_index
5..8    4     chunk_crc32_u32_le
9..12   4     chunk_len_u32_le
```

## Command reference

Each command is annotated with a confidence level:

- `Confirmed`: traced through a complete send/receive path.
- `Inferred`: likely from adjacent code and naming, not directly traced.
- `Unknown`: byte layout known but semantics unclear.

### Device/common control

- Sync time (`Confirmed`)
  - `0B 00 01 80 {yy} {mm} {dd} {dow} {HH} {MM} {SS}`
- Eco window (`Confirmed`)
  - `0A 00 02 80 {flag} {start_h} {start_m} {end_h} {end_m} {light}`
- Reset (`Confirmed`)
  - `04 00 03 80`
- Brightness (`Confirmed`)
  - `05 00 04 80 {brightness}`
- Screen-light timeout set (`Confirmed`)
  - `05 00 0F 80 {value}`
- Screen-light timeout read (`Confirmed`)
  - `05 00 0F 80 FF`
- Rotate 180 (`Confirmed`)
  - `05 00 06 80 {00|01}`
- Time indicator enable (`Confirmed`)
  - `05 00 07 80 {00|01}`
- Countdown (`Confirmed`)
  - `07 00 08 80 {mode} {minutes} {seconds}`
  - mode: `0=reset`, `1=start`, `2=pause`, `3=continue`
- Chronograph (`Confirmed`)
  - `05 00 09 80 {mode}`
  - mode: `0=reset`, `1=start`, `2=pause`, `3=continue`
- Scoreboard (`Confirmed`)
  - `08 00 0A 80 {c1_lo} {c1_hi} {c2_lo} {c2_hi}` (u16 little-endian words)
- Mic type (`Confirmed`)
  - `05 00 0B 80 {type}`
- Joint mode (`Confirmed`)
  - `05 00 0C 80 {mode}`
- Full-screen colour (`Confirmed`)
  - `07 00 02 02 {R} {G} {B}`
  - Fire-and-forget (no ACK expected).
  - RGB channels are 0-255. Independent from brightness command.
- Speed (`Confirmed`)
  - `05 00 03 01 {speed}`
- Enter/exit DIY mode (`Confirmed`)
  - `05 00 04 01 {mode}`
- Clock mode/style (`Confirmed`)
  - `08 00 06 01 {flags} {R} {G} {B}`
- Switchplate (`Confirmed`)
  - `05 00 07 01 {00|01}`
- Set password (`Confirmed`)
  - `08 00 04 02 {op} {p1} {p2} {p3}`
- Verify password (`Confirmed`)
  - `07 00 05 02 {p1} {p2} {p3}`
- Get LED type (`Confirmed`)
  - `04 00 01 80`

### Music/rhythm control

- Send image rhythm (`Confirmed`)
  - `06 00 00 02 {value} 01`
- Stop mic rhythm (`Confirmed`)
  - `06 00 00 02 00 00`
- Mic command variant (`Confirmed`)
  - `06 00 0B 80 {mode+1} {value}`

### System commands

- Delete device material (`Confirmed`)
  - `11 00 02 01 0C 00 01 02 03 04 05 06 07 08 09 0A 0B`
- Locate device (encrypted payload) (`Confirmed`)
  - Plain payload before AES: `06 4C 4F 43 41 54 45 00 00 00 00 00 00 00 00 00`
  - Sent after in-place AES cipher transform.

### OTA setup command

- OTA step 1 (`Confirmed`)
  - `0D 00 {ota_type} C0 {pkg_count} {crc32_u32_le} {bin_size_u32_le}`

## High-level command abstraction for implementation

To implement device-specific behaviour cleanly, use this two-layer model.

Layer A: stable client-facing commands

- `sync_time(datetime)`
- `set_brightness(percent_0_to_100)`
- `set_colour(rgb)`
- `set_power(on_off)`
- `set_rotate(enabled)`
- `set_clock_style(...)`
- `countdown(start/pause/continue/reset, mm, ss)`
- `chronograph(start/pause/continue/reset)`
- `upload_text(...)`
- `upload_gif(...)`
- `upload_image(...)`
- `upload_diy_rgb(...)`

Layer B: resolved device profile and encoder choices

- selected GATT profile and notify endpoint
- selected LED type and panel size
- joint mode (for ambiguous shapes)
- text encoder path (8x32, 16x16-class, 32x32, 64x64, 16x64)
- media tail/profile bytes (`[13..15]`) policy

### Canonical model-resolution algorithm

Use this order:

1. Parse scan AD-TLV and validate manufacturer signature (`TR 00 70/71`).
2. Extract identity fields (`shape`, `cid`, `pid`, `reverse`, group/device id,
   `lamp_count`, `lamp_num`).
   - If trailing lamp bytes are missing, keep `shape/cid/pid` (and other
     available leading fields) and mark lamp fields unknown.
3. Resolve provisional LED type:
   - direct from shape when shape is one of `1,2,3,4,6,7,11`
   - unresolved ambiguous for `0x81/0x82/0x83`
4. Apply persisted per-device override for ambiguous shapes.
5. Connect and resolve endpoints (see
   [Profile selection](#profile-selection-in-practice)).
6. Query `Get LED type` (`04 00 01 80`) and update resolved screen type when
   response is valid (`len >= 9`, `b[2]=0x01`, `b[3]=0x80`).
7. If model remains ambiguous, require explicit user/config selection.
8. If a joint mode is needed, send canonical joint mode (`1/2/5/6`) once after
   connect.

### Command routing rules

Device-specific routing rules:

- `set_power(on_off)` -> `Switchplate` (`05 00 07 01 {00|01}`)
- `set_brightness(0..100)` -> `05 00 04 80 {value}`
- `set_colour(rgb)` -> `07 00 02 02 {R} {G} {B}`
- `sync_time(ts)` -> `0B 00 01 80 {yy mm dd dow HH MM SS}`
- `upload_text(...)` -> choose text path by resolved LED type
- `upload_image(...)` -> choose non-DIY (`family=0x02`) vs DIY by selected
  mode/profile
- `upload_diy_rgb(...)` -> enter DIY mode, then 9-byte DIY chunk flow

Text-path rules:

- LED type `2` => use 8x32 text path.
- LED type `1`, `6`, `7` => use 16x16-class text path.
- LED type `3` => 32x32 path (font size parameter, default 32).
- LED type `4` => 64x64 path (font size parameter: 16, 32, or 64).
- LED type `11` => 16x64 path (fixed 16px height, no font size parameter).
- Unknown LED type => fail fast with typed error or explicit fallback policy.

Known per-model quirks:

- `CID=1, PID=10`: screen-light timeout is read on connect.
- `CID=7, PID=1`: one clock style toggle path is disabled.

## Large payload procedures

### Client-side brightness scaling

Clients apply brightness adjustment to media payloads (image, DIY) before
transmission:

```text
pixel = (byte)(pixel * (brightness / 100.0))
```

The first 5 bytes of each payload chunk (header region) are preserved unchanged.
This scaling is NOT applied to simple control commands (colour fill, etc.).

### Text upload

#### Logical payload

```text
[14-byte text metadata] + [glyph stream]
```

Then wrapped in 16-byte text-family chunk headers (`family=0x03`).

#### Text metadata (14 bytes)

```text
offset  size  field
0..1    2     character_count (little-endian u16: [low, high])
2       1     resolution_flag_1
3       1     resolution_flag_2
4       1     mode (text effect / animation style)
5       1     speed
6       1     text_colour_mode
7       1     text_R
8       1     text_G
9       1     text_B
10      1     background_mode
11      1     bg_R
12      1     bg_G
13      1     bg_B
```

Resolution flags by display path:

| Display path | `byte[2]` | `byte[3]` |
| ------------ | --------- | --------- |
| 8x32         | `0x00`    | `0x01`    |
| 16x16        | `0x01`    | `0x01`    |
| 32x32        | `0x01`    | `0x01`    |
| 64x64        | `0x01`    | `0x01`    |
| 16x64        | `0x00`    | `0x01`    |

For 8x32 panels (`LedType == 2`), the `mode` value MUST be incremented by 1
before encoding.

Colour guard: if `text_R == 0` and `text_G == 0`, then `text_B` MUST be at least
`1`; implementations MUST clamp `0` to `1`.

#### Glyph stream

Each character is encoded as a 4-byte prefix followed by bitmap data. The prefix
format and bitmap size depend on the display path and character classification.

##### 8x32 path (`sendTextTo832`)

Characters are rendered as 12x12 bitmaps, packed via `getTextData12`. Per
character:

```text
[type, pad, pad, pad, ...bitmap_bytes]
```

| Type   | Padding bytes    | Bitmap size | Used for                          |
| ------ | ---------------- | ----------- | --------------------------------- |
| `0x04` | `0xFF 0xFF 0xFF` | 8 bytes     | ASCII/Latin with built-in font    |
| `0x00` | `0x00 0x00 0x00` | 8 bytes     | Rasterised character (compact)    |
| `0x01` | `0x00 0x00 0x00` | 24 bytes    | Rasterised character (12x16 grid) |

Type `0x04` is used for characters that have a hardcoded 8-byte font table entry
(`Text1664.getCharDataByFont`). All other characters are rasterised on the
client at 12x12 pixels and produce type `0x00` (if result is 8 bytes) or `0x01`
(otherwise, typically 24 bytes for CJK on a 16-wide grid).

##### 16x16 path (`sendTextTo1616`)

Characters are rendered at the appropriate size and packed via `getTextData`.
Per character:

```text
[type, 0xFF, 0xFF, 0xFF, ...bitmap_bytes]
```

| Type   | Bitmap size | Used for                              |
| ------ | ----------- | ------------------------------------- |
| `0x02` | 16 bytes    | 8x16 character (ASCII/Latin)          |
| `0x03` | 32 bytes    | 16x16 character (CJK/Japanese/Korean) |

Width selection: CJK, Japanese, and Korean characters use 16px width (32-byte
bitmap, type `0x03`). All others use 8px width (16-byte bitmap, type `0x02`).

##### 32x32 path (`sendTextTo3232`)

This path accepts a font size parameter that controls rendering resolution.
Supported values are 16 and 32 (default: 16). When font size is 16, characters
are rendered at 8x16 / 16x16 (identical to the 16x16-class glyphs). When font
size is 32, characters are rendered at full resolution. Per character:

```text
[type, 0xFF, 0xFF, 0xFF, ...bitmap_bytes]
```

| Type   | Bitmap size | Used for                              |
| ------ | ----------- | ------------------------------------- |
| `0x02` | 16 bytes    | 8x16 character (Latin, font size 16)  |
| `0x03` | 32 bytes    | 16x16 character (CJK, font size 16)   |
| `0x05` | 64 bytes    | 16x32 character (Latin, font size 32) |
| `0x06` | 128 bytes   | 32x32 character (CJK, font size 32)   |

Width selection at font size 32: `getText32Width` returns 32 for
CJK/Japanese/Korean, 16 for all others.

##### 64x64 path (`sendTextTo6464`)

This path accepts a font size parameter with three levels: 16, 32, or 64
(default: 16). It includes all glyph types from the 32x32 path plus two larger
sizes for font size 64. Per character:

```text
[type, 0xFF, 0xFF, 0xFF, ...bitmap_bytes]
```

| Type   | Bitmap size | Used for                              |
| ------ | ----------- | ------------------------------------- |
| `0x02` | 16 bytes    | 8x16 character (Latin, font size 16)  |
| `0x03` | 32 bytes    | 16x16 character (CJK, font size 16)   |
| `0x05` | 64 bytes    | 16x32 character (Latin, font size 32) |
| `0x06` | 128 bytes   | 32x32 character (CJK, font size 32)   |
| `0x07` | 256 bytes   | 32x64 character (Latin, font size 64) |
| `0x08` | 512 bytes   | 64x64 character (CJK, font size 64)   |

Width selection at font size 64: `getText64Width` returns 64 for
CJK/Japanese/Korean, 32 for all others.

##### 16x64 path (`sendTextTo1664`)

Fixed 16px font height. No font size parameter. Glyph type tags are identical to
the 16x16 path. Per character:

```text
[type, 0xFF, 0xFF, 0xFF, ...bitmap_bytes]
```

| Type   | Bitmap size | Used for                              |
| ------ | ----------- | ------------------------------------- |
| `0x02` | 16 bytes    | 8x16 character (Latin)                |
| `0x03` | 32 bytes    | 16x16 character (CJK/Japanese/Korean) |

Width selection: identical to the 16x16 path (`getText16Width`).

##### Glyph type tag summary

All paths share the same type numbering scheme — higher font sizes add new tags
while keeping the lower-resolution tags available:

| Tag    | Width x Height | Total bytes |
| ------ | -------------- | ----------- |
| `0x00` | 8x8 (compact)  | 8           |
| `0x01` | 12x16 (grid)   | 24          |
| `0x02` | 8x16           | 16          |
| `0x03` | 16x16          | 32          |
| `0x04` | 8x8 (font)     | 8           |
| `0x05` | 16x32          | 64          |
| `0x06` | 32x32          | 128         |
| `0x07` | 32x64          | 256         |
| `0x08` | 64x64          | 512         |

Tags `0x00`, `0x01`, and `0x04` are exclusive to the 8x32 path. Tags `0x05` and
above are exclusive to paths with font size >= 32.

#### Bitmap encoding

Bitmaps are 1-bit-per-pixel. Pixels are scanned in row-major order:
`index = y * width + x`. Each group of 8 sequential pixels is packed into one
byte with the first pixel at the LSB:

```text
byte = (px0 << 0) | (px1 << 1) | ... | (px7 << 7)
```

where `px` is `1` if the source pixel is non-zero, `0` otherwise.

For the 12x12 path (`getTextData12`), the bitmap is stored on a 16-pixel-wide
grid (12 data columns + 4 padding columns), yielding `(16 x 12) / 8 = 24` bytes
for a full CJK character.

#### Hardcoded 8x8 font

`Text1664.getCharDataByFont` returns 8-byte bitmaps for: `A-Z`, `a-z`, `0-9`,
space, and common punctuation. Each byte is one row (top to bottom), MSB on the
left. Example: `A` = `{0x00, 0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11}`.

Characters not in this table return `null` and are rasterised instead.

#### Font index for rasterised characters

| Script   | Font index                |
| -------- | ------------------------- |
| Japanese | 9                         |
| Korean   | 8                         |
| Other    | user-selected `fontIndex` |

#### Chunk header tail bytes

Bytes 13-15 of the 16-byte chunk header for text uploads. This pattern is
consistent across all text paths (8x32, 16x16, 32x32, 64x64, 16x64):

| Byte | Simple text | Scheduled/material text |
| ---- | ----------- | ----------------------- |
| 13   | `0x00`      | time parameter (low)    |
| 14   | `0x00`      | time parameter (high)   |
| 15   | `0x0C`      | material slot index     |

For DeviceMaterial and Material overloads, the tail byte (byte 15) is set from
the caller-supplied parameter rather than the fixed `0x0C` value.

#### CRC32 scope

CRC32 is computed over the entire logical payload (14-byte metadata + glyph
stream). The 16-byte chunk headers are excluded.

#### Chunking and send procedure

1. Build logical payload (14-byte metadata + glyph stream).
2. Compute CRC32 over the logical payload.
3. Split logical payload into <=4096-byte segments.
4. Prepend each segment with a 16-byte chunk header (`family=0x03`,
   `chunk_flag=0x00` for first, `0x02` for continuation).
5. Split each (header + segment) into MTU-sized BLE writes (509 or 18 bytes).
6. Send MTU fragments for the first 4K segment at 50 ms intervals.
7. Wait for device ACK notification before sending the next segment.
8. A 5-second timeout aborts the transfer if no ACK arrives.

#### Text ACK parsing

Responses are matched on bytes `[1..4]` of the notification payload:

| `[1]`  | `[2]`  | `[3]`  | `[4]`  | Meaning                    |
| ------ | ------ | ------ | ------ | -------------------------- |
| `0x00` | `0x03` | `0x00` | `0x01` | Send next 4K segment       |
| `0x00` | `0x03` | `0x00` | `0x03` | Transfer complete          |
| `0x00` | `0x03` | `0x00` | `0x02` | Error (insufficient space) |

### GIF upload

- Input payload is raw GIF file bytes. Animation frames are preserved as encoded
  in the source GIF.
- Compute CRC32 over the full GIF payload.
- Split GIF payload into `4096`-byte logical chunks.
- For each logical chunk, prepend a 16-byte media header:
  - `family=0x01`
  - `chunk_flag`: first `0x00`, continuation `0x02`
  - `total_payload_len_u32_le`: full GIF payload length
  - `crc32_u32_le`: full GIF payload CRC
- Tail bytes follow the
  [shared media tail pattern](#shared-media-tail-byte-pattern-gif-image-text).
- Split each `(header + logical_chunk)` block into transport fragments:
  - `509` bytes when MTU-ready
  - `18` bytes fallback

#### GIF send pacing and ACK-driven flow control

- Transport fragment send interval SHOULD be `20 ms` per fragment.
- After finishing the fragments for one logical 4K chunk, sender MUST wait for a
  GIF-family notify status before continuing.
- ACK/status mapping for GIF family:
  - `... 00 01 00 01` => next logical chunk
  - `... 00 01 00 03` => transfer complete
  - `... 00 01 00 02` => transfer error (for example space)
  - `... 00 01 00 00` => invalid-command style error
- Sender SHOULD apply a `5 s` ACK timeout per logical chunk and fail transfer on
  timeout.

#### Normative GIF upload algorithm

1. Validate non-empty GIF payload.
2. Compute `crc32` over full payload.
3. Split payload into `<=4096` logical chunks.
4. For each logical chunk, build 16-byte header with `family=0x01` and shared
   tail-byte policy.
5. For each `(header + logical_chunk)`, split to transport fragments (509/18).
6. Send fragments for current logical chunk at `20 ms` interval.
7. Wait up to `5 s` for GIF-family notify:
   - on `status=0x01`, continue with next logical chunk;
   - on `status=0x03`, mark success and stop;
   - on `status=0x02` or `status=0x00`, fail and stop.
8. If timeout occurs before status, fail and stop.
9. On disconnect/Bluetooth-off during transfer, fail and stop.

#### Shared media tail byte pattern (GIF, Image, Text)

All 16-byte media header families use the same tail byte logic for bytes
`[13..15]`. The caller passes a slot parameter `i`:

| Condition | Byte 13    | Byte 14     | Byte 15    |
| --------- | ---------- | ----------- | ---------- |
| `i == 12` | `0x00`     | `0x00`      | `0x0C`     |
| `i != 12` | time (low) | time (high) | `(byte) i` |

When `i == 12` (`0x0C`), this is a simple/immediate transfer with no scheduling
metadata. When `i` is another value, byte 15 acts as a material slot index and
bytes 13-14 carry a display duration as u16_le.

The duration value comes from `DeviceMaterialTimeConvert.ConvertTime(timeSign)`:

| Time sign | Duration (seconds) | UI label |
| --------- | ------------------ | -------- |
| 0         | 5                  | 5s       |
| 1         | 10                 | 10s      |
| 2         | 30                 | 30s      |
| 3         | 60                 | 1min     |
| 4         | 300                | 5min     |

The time sign is a user preference stored per material slot.

### Display-intent payload semantics

For static image display uploads, clients SHOULD treat both `image` and bulk
`DIY` payloads as RGB888 framebuffers:

- Byte layout per pixel: `[R, G, B]`.
- Pixel order: row-major, top-to-bottom then left-to-right.
- Expected payload length: `panel_width * panel_height * 3`.

Before payload encoding, clients SHOULD normalise source media to panel geometry
(resize/crop and EXIF rotation) and then convert the normalised bitmap into the
RGB888 stream above.

### Image upload (non-DIY)

- Uses the same 16-byte framing shape as GIF/text.
- `family=0x02`.
- Display-intent payload SHOULD be RGB888 framebuffer bytes as defined above.
- CRC and total length are carried in header bytes `[5..12]`.
- Tail bytes follow the
  [shared media tail pattern](#shared-media-tail-byte-pattern-gif-image-text).

### DIY raw RGB upload

- Mode switch command sent before transfer (`05 00 04 01 01`).
- Bulk payload uses the same RGB888 framebuffer layout as image upload.
- Split payload into `4096`-byte chunks.
- Prefix each chunk with 9-byte DIY header.

### Timer transfer protocols

Timer flows include at least:

- image payload transfer (24-byte header)
- text payload transfer (24-byte header)
- close/disable command (12-byte control frame)

#### Timer text device-specific branching

When a timer carries text content, the text metadata and glyph encoding follow
the same rules as [Text upload](#text-upload). One additional LED-type-specific
rule applies:

- When `LedType == 2` (8x32): the text `mode` value is incremented by 1 before
  encoding into the metadata, matching the regular text upload behaviour.

The timer text metadata uses resolution flags `[0, 1]` (same as 8x32/16x64)
regardless of LED type.

### Schedule transfer protocols

Schedule theme transfers include:

- GIF themes (`theme_type=1`)
- image themes (`theme_type=2`)
- text themes (`theme_type=3`)

Transfers are queued and paced by setup acknowledgements plus master-switch
control.

#### Schedule text device-specific branching

Schedule text content uses a different glyph rendering path based on LED type:

**LED type 2 (8x32) or 11 (16x64)**: Uses 8x32-class rendering.

- Characters with a hardcoded font entry use type tag `0x04` (8-byte bitmap).
- Other characters are rasterised at 12x12 via `getCharBitmap8X32` +
  `getTextData12`, producing type `0x00` (8 bytes) or `0x01` (24 bytes).
- This matches the regular 8x32 text path [glyph encoding](#glyph-stream).

**All other LED types**: Uses 16x16-class rendering.

- Characters are rendered via `getCharBitmap` at 8x16 or 16x16.
- Type tag `0x02` for 16-byte bitmaps (8x16), `0x03` for 32-byte bitmaps
  (16x16).
- Width selection uses `isChinese()` only (not Japanese/Korean), which differs
  from the regular 16x16 text path that also checks Japanese and Korean for
  full-width treatment.

Resolution flags for schedule text are `[0, 1]` regardless of LED type.

Note: unlike the regular text and timer text paths, the schedule path does NOT
increment the text mode for LED type 2.

### OTA transfer protocol

- Step-1 command announces package count, whole-file CRC, and binary size.
- Device acknowledgement triggers chunk transfer.
- Binary is split to `4096` chunks.
- Each chunk is wrapped in 13-byte OTA header and ATT-chunked for BLE writes.

## Notify responses and flow control

All transfer ACK callbacks check `length >= 5` before parsing. The family is
identified by bytes `[1..3]` and the status is at byte `[4]`.

### Transfer family ACK patterns

Byte layout: `{len_lo} {[1]} {[2]} {[3]} {[4]=status}`

**Text** — `[1]=0x00 [2]=0x03 [3]=0x00`:

| `[4]`  | Meaning      | Handler action             |
| ------ | ------------ | -------------------------- |
| `0x01` | Next package | Send next 4K segment       |
| `0x02` | Error        | Abort (insufficient space) |
| `0x03` | Finish       | Transfer complete          |

**GIF** — `[1]=0x00 [2]=0x01 [3]=0x00`:

| `[4]`  | Meaning      | Handler action                      |
| ------ | ------------ | ----------------------------------- |
| `0x00` | Invalid      | Abort (invalid-command style error) |
| `0x01` | Next package | Send next 4K segment                |
| `0x02` | Error        | Abort (insufficient space)          |
| `0x03` | Finish       | Transfer complete                   |

**Image** — `[1]=0x00 [2]=0x02 [3]=0x00`:

| `[4]`  | Meaning      | Handler action             |
| ------ | ------------ | -------------------------- |
| `0x01` | Next package | Send next 4K segment       |
| `0x02` | Error        | Abort (insufficient space) |
| `0x03` | Finish       | Transfer complete          |

**DIY** — `[1]=0x00 [2]=0x00 [3]=0x00`:

| `[4]`  | Meaning      | Handler action       |
| ------ | ------------ | -------------------- |
| `0x00` | Finish (alt) | Transfer complete    |
| `0x01` | Finish       | Transfer complete    |
| `0x02` | Next package | Send next 4K segment |

Note: DIY uses `0x02` for next (not `0x01`), and both `0x00` and `0x01` signal
completion.

**Timer** — `[1]=0x00 [2]=0x00 [3]=0x80`:

| `[4]`  | Meaning       | Handler action                      |
| ------ | ------------- | ----------------------------------- |
| `0x00` | Failure       | Abort with error                    |
| `0x01` | Next/complete | Send next segment or finish if done |
| `0x03` | Save success  | Data saved to device                |

Note: Timer uses `0x01` for both "next chunk" and "complete" — the handler
checks whether more data remains to distinguish the two cases.

**OTA** — `[1]=0x00 [2]=0x01 [3]=0xC0`:

| `[4]`  | Meaning      | Handler action        |
| ------ | ------------ | --------------------- |
| `0x00` | Error        | Abort                 |
| `0x01` | Next package | Send next OTA chunk   |
| `0x03` | Finish       | OTA transfer complete |

### OTA step-1 ACK

Before chunk transfer begins, the device must acknowledge the OTA start command.
Two response variants are accepted:

- `05 00 00 80 01`
- `05 00 02 80 01`

Byte `[2]` may be `0x00` or `0x02`; both are valid. Any other response (or
timeout) is treated as rejection.

### Schedule control responses

Schedule responses use a different pattern — they check byte `[0]` and use a
verification method rather than the standard `[1..4]` dispatch.

**Setup response** — `[0]=0x05 [1]=0x00 [2]=0x05 [3]=0x80`:

| `[4]`  | Meaning  | Handler action                       |
| ------ | -------- | ------------------------------------ |
| `0x01` | Success  | Proceed to send next queued resource |
| `0x03` | Continue | Load next resource queue item        |
| other  | Failure  | Abort with error                     |

**Master-switch response** — `[0]=0x05 [1]=0x00 [2]=0x07 [3]=0x80`:

| `[4]`  | Meaning | Handler action              |
| ------ | ------- | --------------------------- |
| `0x01` | Success | Begin sending schedule data |
| other  | Failure | Abort with error            |

### Family dispatch summary

| Family   | `[1]`  | `[2]`            | `[3]`  | Status byte |
| -------- | ------ | ---------------- | ------ | ----------- |
| Text     | `0x00` | `0x03`           | `0x00` | `[4]`       |
| GIF      | `0x00` | `0x01`           | `0x00` | `[4]`       |
| Image    | `0x00` | `0x02`           | `0x00` | `[4]`       |
| DIY      | `0x00` | `0x00`           | `0x00` | `[4]`       |
| Timer    | `0x00` | `0x00`           | `0x80` | `[4]`       |
| OTA      | `0x00` | `0x01`           | `0xC0` | `[4]`       |
| Schedule | `0x00` | `0x05` or `0x07` | `0x80` | `[4]`       |

Implementations SHOULD parse responses by command family rather than one
universal ACK map.

## Compatibility notes

- The only known implementation always uses FA/FA02 (see
  [Profile selection](#profile-selection-in-practice)), but the library layer
  contains FEE9/D44 defaults. Devices using the FEE9 profile may exist on older
  firmware.
- Notify endpoints are auto-detected by GATT property flags, not by UUID.
- Large-payload flows are notification-driven; fire-and-forget is only used for
  short control commands (e.g. colour fill, brightness).
- Transfer timeouts are per family (typically 5 seconds per 4K segment).
