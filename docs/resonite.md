# Resonite OSC Face Tracking Implementation Guide

## Quick Reference: Essential Information

**Protocol:** OSC over UDP  
**Default Port:** 9000 or 9015 (configure in Resonite Settings > Devices)  
**Target IP:** 127.0.0.1 (localhost) or network IP for remote devices  
**Update Rate:** 60-90 Hz recommended  
**Data Type:** Float (0.0 to 1.0 for most parameters, -1.0 to 1.0 for eye directions)

## Network Configuration

| Parameter                     | Value             | Description                                        |
| ----------------------------- | ----------------- | -------------------------------------------------- |
| **Protocol**                  | UDP               | OSC uses UDP for low-latency transmission          |
| **Default Port**              | 9000 or 9015      | Configurable in Settings > Devices                 |
| **Port (Steam Link Custom)**  | 9015              | When using "Custom" mode in Steam Link             |
| **Port (Steam Link Default)** | 9000              | Default port for Steam Link OSC                    |
| **IP Address**                | 127.0.0.1 (local) | Typically localhost for same-machine communication |

### Resonite Configuration Steps

1. **In Resonite Settings:**

   - Go to Settings > Devices
   - Set "OSC Face Tracking Port for Steam Link" to your chosen port (9000 or 9015)
   - This port is where Resonite will listen for incoming OSC messages

2. **Your Application Configuration:**
   - Configure your OSC sender to transmit to `127.0.0.1` (localhost) if running on same machine
   - Use the same port configured in Resonite (9000 or 9015)
   - Send UDP packets with proper OSC encoding
   - Maintain 60-90 Hz update rate for smooth tracking

## Data Structure Specification

### Supported Data Types

| Type       | Description                       | Usage                                           |
| ---------- | --------------------------------- | ----------------------------------------------- |
| `Float`    | Single-precision floating point   | Most common for tracking data (0.0 - 1.0 range) |
| `Double`   | Double-precision floating point   | High-precision values                           |
| `Int`      | 32-bit integer                    | Discrete values                                 |
| `Long`     | 64-bit integer                    | Large discrete values                           |
| `String`   | Text data                         | Rarely used for tracking                        |
| `Color`    | RGBA color values                 | Special use cases                               |
| `ColorX`   | Extended color (converts to sRGB) | Color manipulation                              |
| `DateTime` | Timestamp data                    | Temporal synchronization                        |
| `Byte`     | 8-bit unsigned integer            | Compact data transmission                       |

**Note:** Most face and eye tracking applications send data as `Float` values normalized between 0.0 and 1.0.

### Multi-Dimensional Values

OSC supports multi-dimensional data (similar to Resonite's Float3 type). Access individual components using the `ArgumentIndex`:

`/osc/rotation` → `[pitch, yaw, roll]`

- ArgumentIndex 0 = Pitch (90.0)
- ArgumentIndex 1 = Yaw (180.0)
- ArgumentIndex 2 = Roll (0.0)

## Implementation Steps

### 1. Setup UDP Socket

```csharp
socket = create_udp_socket()
target_address = "127.0.0.1" // or remote IP
target_port = 9015 // or 9000
```

### 2. Encode OSC Messages

**OSC Packet Format:**

- Address pattern (null-terminated string, 4-byte aligned)
- Type tag string (starts with `,`, null-terminated, 4-byte aligned)
- Arguments (4-byte aligned)

**Float encoding:** Big-endian IEEE 754 32-bit float

### 3. Send at Regular Intervals

```csharp
loop every 16.67ms { // 60 Hz
    tracking_data = get_face_tracking()

    // Bundle messages for efficiency
    bundle = create_osc_bundle()
    bundle.add("/sl/xrfb/facew/JawDrop", tracking_data.jaw_open)
    bundle.add("/avatar/parameters/LeftEyeX", tracking_data.left_eye_x)
    // ... add all parameters

    send_udp(socket, bundle, target_address, target_port)
}
```

### 4. Normalize Input Data

**Critical:** Always normalize to correct ranges:

```csharp
// Eye X/Y: Device range → -1.0 to 1.0
normalized_eye_x = (raw_eye_x - center) / max_range
normalized_eye_x = clamp(normalized_eye_x, -1.0, 1.0)

// Face expressions: Device range → 0.0 to 1.0
normalized_expression = raw_value / max_value
normalized_expression = clamp(normalized_expression, 0.0, 1.0)

// Apply smoothing (exponential moving average)
smoothed = (new_value * 0.3) + (previous_value * 0.7)

// Apply dead zone
if abs(smoothed) < 0.03:
    smoothed = 0.0
```

## Performance Considerations

### Data Rate Recommendations

- **Update Frequency:** 60-90 Hz for smooth tracking (90 Hz recommended for eyes, 60 Hz for face)
- **Average Bundle Size:** 20-40 messages per bundle
- **Bandwidth:** ~10-20 KB/s for full face and eye tracking
- **Latency Target:** <10ms from capture to send for responsive tracking

### Optimization Strategies

1. **Prioritized Sending:** Send eye tracking first, face tracking second.
2. **Adaptive Rate:** Reduce update rate when values haven't changed significantly.
3. **Delta Encoding:** Only send parameters that changed by >0.02 since last frame.
4. **Smoothing Before Send:** Apply temporal smoothing to reduce jitter (0.05-0.15 second window).
5. **Dead Zone Implementation:** Don't send values below 0.03-0.05 threshold.
6. **Batch Messages:** Always use bundles instead of individual messages.

## Resonite Internal Expression Mapping

### AvatarExpression Enum

Resonite uses the `AvatarExpression` enum to map OSC data to avatar blendshapes.

| Enum Name              | Value | Description                              | OSC Mapping                     |
| ---------------------- | ----- | ---------------------------------------- | ------------------------------- |
| Smile                  | 0     | Average smile with mouth slightly open   | Combined SmileLeft + SmileRight |
| SmileLeft              | 1     | Left lip corner up, mouth open           | LipCornerPullerL                |
| SmileRight             | 2     | Right lip corner up, mouth open          | LipCornerPullerR                |
| SmirkLeft              | 3     | Left lip corner curl (estimated)         | Derived from smile data         |
| SmirkRight             | 4     | Right lip corner curl (estimated)        | Derived from smile data         |
| Frown                  | 5     | Average frown                            | Combined FrownLeft + FrownRight |
| FrownLeft              | 6     | Left lip corner down                     | LipCornerDepressorL             |
| FrownRight             | 7     | Right lip corner down                    | LipCornerDepressorR             |
| MouthDimple            | 8     | Mouth squish inward (estimated)          | Combined left + right           |
| MouthDimpleLeft        | 9     | Left mouth dimple                        | DimplerL                        |
| MouthDimpleRight       | 10    | Right mouth dimple                       | DimplerR                        |
| TongueOut              | 11    | Tongue sticking out                      | TongueOut                       |
| TongueRaise            | 12    | Tongue curling upwards                   | Derived                         |
| TongueExtend           | 13    | Tongue extension variant                 | TongueOut                       |
| TongueLeft             | 14    | Tongue to left                           | Derived                         |
| TongueRight            | 15    | Tongue to right                          | Derived                         |
| TongueDown             | 16    | Tongue downward                          | TongueRetreat                   |
| TongueUp               | 17    | Tongue upward                            | TongueTipAlveolar               |
| TongueRoll             | 18    | Tongue taco roll                         | Derived                         |
| TongueHorizontal       | 19    | Tongue flatten horizontal                | FrontDorsalPalate               |
| TongueVertical         | 20    | Tongue squish vertical                   | MidDorsalPalate                 |
| TongueUpLeft           | 21    | Tongue left and up                       | Combined                        |
| TongueUpRight          | 22    | Tongue right and up                      | Combined                        |
| TongueDownLeft         | 23    | Tongue left and down                     | Combined                        |
| TongueDownRight        | 24    | Tongue right and down                    | Combined                        |
| SmileClosed            | 25    | Closed smile (estimated)                 | Derived                         |
| SmileClosedLeft        | 26    | Left closed smile (estimated)            | Derived                         |
| SmileClosedRight       | 27    | Right closed smile (estimated)           | Derived                         |
| Grin                   | 28    | Upper lip lift while smiling (estimated) | UpperLipRaiserL/R               |
| GrinLeft               | 29    | Left upper lip grin (estimated)          | UpperLipRaiserL                 |
| GrinRight              | 30    | Right upper lip grin (estimated)         | UpperLipRaiserR                 |
| Angry                  | 31    | Lips lift while frowning (estimated)     | Combined brow + lip             |
| CheekPuffLeft          | 32    | Left cheek puff                          | CheekPuffL                      |
| CheekPuffRight         | 33    | Right cheek puff                         | CheekPuffR                      |
| CheekPuff              | 34    | Both cheeks puff                         | Combined                        |
| CheekSuckLeft          | 35    | Left cheek suck                          | CheekSuckL                      |
| CheekSuckRight         | 36    | Right cheek suck                         | CheekSuckR                      |
| CheekSuck              | 37    | Both cheeks suck                         | Combined                        |
| CheekRaiseLeft         | 38    | Left cheek raise                         | CheekRaiserL                    |
| CheekRaiseRight        | 39    | Right cheek raise                        | CheekRaiserR                    |
| CheekRaise             | 40    | Both cheeks raise                        | Combined                        |
| LipRaiseUpperLeft      | 41    | Left upper lip raise                     | UpperLipRaiserL                 |
| LipRaiseUpperRight     | 42    | Right upper lip raise                    | UpperLipRaiserR                 |
| LipRaiseLowerLeft      | 43    | Left lower lip raise                     | Derived                         |
| LipRaiseLowerRight     | 44    | Right lower lip raise                    | Derived                         |
| LipRaiseUpper          | 45    | Upper lip raise                          | Combined                        |
| LipRaiseLower          | 46    | Lower lip raise                          | Combined                        |
| LipMoveLeftUpper       | 47    | Upper lip shift left                     | MouthLeft                       |
| LipMoveRightUpper      | 48    | Upper lip shift right                    | MouthRight                      |
| LipMoveLeftLower       | 49    | Lower lip shift left                     | MouthLeft                       |
| LipMoveRightLower      | 50    | Lower lip shift right                    | MouthRight                      |
| LipMoveHorizontalUpper | 51    | Upper lip horizontal shift               | Combined left/right             |
| LipMoveHorizontalLower | 52    | Lower lip horizontal shift               | Combined left/right             |
| LipTopLeftOverturn     | 53    | Left upper lip flip up                   | LipFunnelerLT                   |
| LipTopRightOverturn    | 54    | Right upper lip flip up                  | LipFunnelerRT                   |
| LipTopOverturn         | 55    | Upper lip flip up (ape)                  | Combined funnel top             |
| LipBottomLeftOverturn  | 56    | Left lower lip flip down                 | LipFunnelerLB                   |
| LipBottomRightOverturn | 57    | Right lower lip flip down                | LipFunnelerRB                   |
| LipBottomOverturn      | 58    | Lower lip flip down (ape)                | Combined funnel bottom          |
| LipOverlayUpper        | 59    | Upper lip over lower                     | LipsToward                      |
| LipOverlayUpperLeft    | 60    | Left upper lip overlay                   | LipSuckLT                       |
| LipOverlayUpperRight   | 61    | Right upper lip overlay                  | LipSuckRT                       |
| LipUnderlayUpper       | 62    | Upper lip under lower                    | LipSuckLT/RT                    |
| LipUnderlayUpperLeft   | 63    | Left upper lip underlay                  | LipSuckLT                       |
| LipUnderlayUpperRight  | 64    | Right upper lip underlay                 | LipSuckRT                       |
| LipOverlayLower        | 65    | Lower lip over upper                     | LipsToward                      |
| LipOverlayLowerLeft    | 66    | Left lower lip overlay                   | LipSuckLB                       |
| LipOverlayLowerRight   | 67    | Right lower lip overlay                  | LipSuckRB                       |
| LipUnderlayLower       | 68    | Lower lip under upper                    | LipSuckLB/RB                    |
| LipUnderlayLowerLeft   | 69    | Left lower lip underlay                  | LipSuckLB                       |
| LipUnderlayLowerRight  | 70    | Right lower lip underlay                 | LipSuckRB                       |
| LipStretch             | 71    | Lip stretch                              | Combined                        |
| LipStretchLeft         | 72    | Left lip stretch                         | LipStretcherL                   |
| LipStretchRight        | 73    | Right lip stretch                        | LipStretcherR                   |
| LipTighten             | 74    | Lip tighten                              | Combined                        |
| LipTightenLeft         | 75    | Left lip tighten                         | LipTightenerL                   |
| LipTightenRight        | 76    | Right lip tighten                        | LipTightenerR                   |
| LipsPress              | 77    | Lips pressed together                    | Combined                        |
| LipsPressLeft          | 78    | Left lips press                          | LipPressorL                     |
| LipsPressRight         | 79    | Right lips press                         | LipPressorR                     |
| JawLeft                | 80    | Jaw shift left                           | JawSidewaysLeft                 |
| JawRight               | 81    | Jaw shift right                          | JawSidewaysRight                |
| JawHorizontal          | 82    | Jaw horizontal shift                     | Combined left/right             |
| JawForward             | 83    | Jaw forward                              | JawThrust                       |
| JawDown                | 84    | Jaw down, mouth closed                   | Partial JawDrop                 |
| JawOpen                | 85    | Jaw opening                              | JawDrop                         |
| Pout                   | 86    | Kissy face                               | Combined pucker                 |
| PoutLeft               | 87    | Left kissy face                          | LipPuckerL                      |
| PoutRight              | 88    | Right kissy face                         | LipPuckerR                      |
| NoseWrinkle            | 89    | Nose muscle push up (estimated)          | Combined                        |
| NoseWrinkleLeft        | 90    | Left nose wrinkle                        | NoseWrinklerL                   |
| NoseWrinkleRight       | 91    | Right nose wrinkle                       | NoseWrinklerR                   |
| ChinRaise              | 92    | Chin pull up                             | Combined                        |
| ChinRaiseBottom        | 93    | Chin bottom raise                        | ChinRaiserB                     |
| ChinRaiseTop           | 94    | Chin top raise                           | ChinRaiserT                     |

**Note:** Expressions marked as "estimated" can be automatically computed by Resonite when `EstimateIfNotTracked` is enabled on the `AvatarExpressionDriver`.

## Troubleshooting

### Testing Your OSC Sender

1. **Test UDP connectivity:**

   ```powershell
   # Windows PowerShell
   Test-NetConnection -ComputerName 127.0.0.1 -Port 9015
   ```

   ```bash
   # Linux/Mac
   nc -zu 127.0.0.1 9015
   ```

2. **Monitor OSC traffic with Wireshark:**

   - Filter: `udp.port == 9015`
   - Verify packets are being sent and check structure.

3. **Common Issues:**
   - **No Data:** Verify port in Resonite Settings > Devices. Check Firewall.
   - **Incorrect Mapping:** OSC paths are case-sensitive (e.g., `/sl/xrfb/facew/JawDrop`).
   - **Jitter:** Implement smoothing and dead zones (0.03 threshold).

## Appendix: Complete OSC Address Reference

### All 78 Tracking Parameters

| Category       | OSC Address                           | Range       | Description                             |
| :------------- | :------------------------------------ | :---------- | :-------------------------------------- |
| **Eye**        | `/avatar/parameters/LeftEyeX`         | -1.0 to 1.0 | Left eye horizontal rotation            |
| **Eye**        | `/avatar/parameters/LeftEyeY`         | -1.0 to 1.0 | Left eye vertical rotation              |
| **Eye**        | `/avatar/parameters/RightEyeX`        | -1.0 to 1.0 | Right eye horizontal rotation           |
| **Eye**        | `/avatar/parameters/RightEyeY`        | -1.0 to 1.0 | Right eye vertical rotation             |
| **Eye**        | `/avatar/parameters/LeftEyeLid`       | 0.0 to 1.0  | Left eyelid closure (0=open, 1=closed)  |
| **Eye**        | `/avatar/parameters/RightEyeLid`      | 0.0 to 1.0  | Right eyelid closure (0=open, 1=closed) |
| **Eye**        | `/sl/xrfb/facew/EyesClosedL`          | 0.0 to 1.0  | Left eye closure weight                 |
| **Eye**        | `/sl/xrfb/facew/EyesClosedR`          | 0.0 to 1.0  | Right eye closure weight                |
| **Eye**        | `/sl/xrfb/facew/EyesLookDownL`        | 0.0 to 1.0  | Left eye downward gaze                  |
| **Eye**        | `/sl/xrfb/facew/EyesLookDownR`        | 0.0 to 1.0  | Right eye downward gaze                 |
| **Eye**        | `/sl/xrfb/facew/EyesLookLeftL`        | 0.0 to 1.0  | Left eye leftward gaze                  |
| **Eye**        | `/sl/xrfb/facew/EyesLookLeftR`        | 0.0 to 1.0  | Right eye leftward gaze                 |
| **Eye**        | `/sl/xrfb/facew/EyesLookRightL`       | 0.0 to 1.0  | Left eye rightward gaze                 |
| **Eye**        | `/sl/xrfb/facew/EyesLookRightR`       | 0.0 to 1.0  | Right eye rightward gaze                |
| **Eye**        | `/sl/xrfb/facew/EyesLookUpL`          | 0.0 to 1.0  | Left eye upward gaze                    |
| **Eye**        | `/sl/xrfb/facew/EyesLookUpR`          | 0.0 to 1.0  | Right eye upward gaze                   |
| **Brow**       | `/sl/xrfb/facew/BrowLowererL`         | 0.0 to 1.0  | Left eyebrow lowering                   |
| **Brow**       | `/sl/xrfb/facew/BrowLowererR`         | 0.0 to 1.0  | Right eyebrow lowering                  |
| **Brow**       | `/sl/xrfb/facew/InnerBrowRaiserL`     | 0.0 to 1.0  | Left inner eyebrow raise                |
| **Brow**       | `/sl/xrfb/facew/InnerBrowRaiserR`     | 0.0 to 1.0  | Right inner eyebrow raise               |
| **Brow**       | `/sl/xrfb/facew/OuterBrowRaiserL`     | 0.0 to 1.0  | Left outer eyebrow raise                |
| **Brow**       | `/sl/xrfb/facew/OuterBrowRaiserR`     | 0.0 to 1.0  | Right outer eyebrow raise               |
| **Cheek**      | `/sl/xrfb/facew/CheekPuffL`           | 0.0 to 1.0  | Left cheek puff                         |
| **Cheek**      | `/sl/xrfb/facew/CheekPuffR`           | 0.0 to 1.0  | Right cheek puff                        |
| **Cheek**      | `/sl/xrfb/facew/CheekRaiserL`         | 0.0 to 1.0  | Left cheek raise                        |
| **Cheek**      | `/sl/xrfb/facew/CheekRaiserR`         | 0.0 to 1.0  | Right cheek raise                       |
| **Cheek**      | `/sl/xrfb/facew/CheekSuckL`           | 0.0 to 1.0  | Left cheek suck                         |
| **Cheek**      | `/sl/xrfb/facew/CheekSuckR`           | 0.0 to 1.0  | Right cheek suck                        |
| **Eyelid**     | `/sl/xrfb/facew/LidTightenerL`        | 0.0 to 1.0  | Left eyelid tightening                  |
| **Eyelid**     | `/sl/xrfb/facew/LidTightenerR`        | 0.0 to 1.0  | Right eyelid tightening                 |
| **Eyelid**     | `/sl/xrfb/facew/UpperLidRaiserL`      | 0.0 to 1.0  | Left upper eyelid raise                 |
| **Eyelid**     | `/sl/xrfb/facew/UpperLidRaiserR`      | 0.0 to 1.0  | Right upper eyelid raise                |
| **Jaw**        | `/sl/xrfb/facew/JawDrop`              | 0.0 to 1.0  | Jaw opening                             |
| **Jaw**        | `/sl/xrfb/facew/JawSidewaysLeft`      | 0.0 to 1.0  | Jaw movement left                       |
| **Jaw**        | `/sl/xrfb/facew/JawSidewaysRight`     | 0.0 to 1.0  | Jaw movement right                      |
| **Jaw**        | `/sl/xrfb/facew/JawThrust`            | 0.0 to 1.0  | Jaw forward thrust                      |
| **Lip**        | `/sl/xrfb/facew/LipCornerDepressorL`  | 0.0 to 1.0  | Left lip corner down                    |
| **Lip**        | `/sl/xrfb/facew/LipCornerDepressorR`  | 0.0 to 1.0  | Right lip corner down                   |
| **Lip**        | `/sl/xrfb/facew/LipCornerPullerL`     | 0.0 to 1.0  | Left lip corner pull (smile)            |
| **Lip**        | `/sl/xrfb/facew/LipCornerPullerR`     | 0.0 to 1.0  | Right lip corner pull (smile)           |
| **Lip**        | `/sl/xrfb/facew/LipFunnelerLB`        | 0.0 to 1.0  | Left bottom lip funnel                  |
| **Lip**        | `/sl/xrfb/facew/LipFunnelerLT`        | 0.0 to 1.0  | Left top lip funnel                     |
| **Lip**        | `/sl/xrfb/facew/LipFunnelerRB`        | 0.0 to 1.0  | Right bottom lip funnel                 |
| **Lip**        | `/sl/xrfb/facew/LipFunnelerRT`        | 0.0 to 1.0  | Right top lip funnel                    |
| **Lip**        | `/sl/xrfb/facew/LipPressorL`          | 0.0 to 1.0  | Left lip press                          |
| **Lip**        | `/sl/xrfb/facew/LipPressorR`          | 0.0 to 1.0  | Right lip press                         |
| **Lip**        | `/sl/xrfb/facew/LipPuckerL`           | 0.0 to 1.0  | Left lip pucker                         |
| **Lip**        | `/sl/xrfb/facew/LipPuckerR`           | 0.0 to 1.0  | Right lip pucker                        |
| **Lip**        | `/sl/xrfb/facew/LipStretcherL`        | 0.0 to 1.0  | Left lip stretch                        |
| **Lip**        | `/sl/xrfb/facew/LipStretcherR`        | 0.0 to 1.0  | Right lip stretch                       |
| **Lip**        | `/sl/xrfb/facew/LipSuckLB`            | 0.0 to 1.0  | Left bottom lip suck                    |
| **Lip**        | `/sl/xrfb/facew/LipSuckLT`            | 0.0 to 1.0  | Left top lip suck                       |
| **Lip**        | `/sl/xrfb/facew/LipSuckRB`            | 0.0 to 1.0  | Right bottom lip suck                   |
| **Lip**        | `/sl/xrfb/facew/LipSuckRT`            | 0.0 to 1.0  | Right top lip suck                      |
| **Lip**        | `/sl/xrfb/facew/LipTightenerL`        | 0.0 to 1.0  | Left lip tightening                     |
| **Lip**        | `/sl/xrfb/facew/LipTightenerR`        | 0.0 to 1.0  | Right lip tightening                    |
| **Lip**        | `/sl/xrfb/facew/LipsToward`           | 0.0 to 1.0  | Lips moving toward each other           |
| **Lip**        | `/sl/xrfb/facew/LowerLipDepressorL`   | 0.0 to 1.0  | Left lower lip depression               |
| **Lip**        | `/sl/xrfb/facew/LowerLipDepressorR`   | 0.0 to 1.0  | Right lower lip depression              |
| **Lip**        | `/sl/xrfb/facew/UpperLipRaiserL`      | 0.0 to 1.0  | Left upper lip raise                    |
| **Lip**        | `/sl/xrfb/facew/UpperLipRaiserR`      | 0.0 to 1.0  | Right upper lip raise                   |
| **Lip**        | `/sl/xrfb/facew/MouthLeft`            | 0.0 to 1.0  | Mouth horizontal left                   |
| **Lip**        | `/sl/xrfb/facew/MouthRight`           | 0.0 to 1.0  | Mouth horizontal right                  |
| **Nose/Chin**  | `/sl/xrfb/facew/ChinRaiserB`          | 0.0 to 1.0  | Bottom chin raise                       |
| **Nose/Chin**  | `/sl/xrfb/facew/ChinRaiserT`          | 0.0 to 1.0  | Top chin raise                          |
| **Nose/Chin**  | `/sl/xrfb/facew/DimplerL`             | 0.0 to 1.0  | Left mouth dimple                       |
| **Nose/Chin**  | `/sl/xrfb/facew/DimplerR`             | 0.0 to 1.0  | Right mouth dimple                      |
| **Nose/Chin**  | `/sl/xrfb/facew/NoseWrinklerL`        | 0.0 to 1.0  | Left nose wrinkle                       |
| **Nose/Chin**  | `/sl/xrfb/facew/NoseWrinklerR`        | 0.0 to 1.0  | Right nose wrinkle                      |
| **Tongue**     | `/sl/xrfb/facew/TongueTipInterdental` | 0.0 to 1.0  | Tongue between teeth                    |
| **Tongue**     | `/sl/xrfb/facew/TongueTipAlveolar`    | 0.0 to 1.0  | Tongue tip at alveolar ridge            |
| **Tongue**     | `/sl/xrfb/facew/FrontDorsalPalate`    | 0.0 to 1.0  | Front dorsal tongue position            |
| **Tongue**     | `/sl/xrfb/facew/MidDorsalPalate`      | 0.0 to 1.0  | Mid dorsal tongue position              |
| **Tongue**     | `/sl/xrfb/facew/BackDorsalVelar`      | 0.0 to 1.0  | Back dorsal tongue position             |
| **Tongue**     | `/sl/xrfb/facew/TongueOut`            | 0.0 to 1.0  | Tongue extension outward                |
| **Tongue**     | `/sl/xrfb/facew/TongueRetreat`        | 0.0 to 1.0  | Tongue retraction                       |
| **Confidence** | `/sl/xrfb/facec/LowerFace`            | 0.0 to 1.0  | Lower face tracking confidence          |
| **Confidence** | `/sl/xrfb/facec/UpperFace`            | 0.0 to 1.0  | Upper face tracking confidence          |

## References

[1] Resonite Wiki - OSC Documentation. https://wiki.resonite.com/OSC  
[2] Resonite Wiki - Face and Eye Tracking Troubleshooting. https://wiki.resonite.com/Face_and_Eye_Tracking_(Troubleshooting)  
[3] Resonite Wiki - AvatarExpressionDriver Component. https://wiki.resonite.com/Component:AvatarExpressionDriver  
[4] Open Sound Control Specification. http://opensoundcontrol.org/spec-1_0
