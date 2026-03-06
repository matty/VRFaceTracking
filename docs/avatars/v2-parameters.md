# V2 Parameters

V2 parameters are the recommended parameter set for new avatars. They use the `v2/` OSC address prefix and provide full coverage of the unified expression format.

## OSC Address Format

vrft_d sends each expression as up to three sub-parameters:

| Sub-param | Type | Address example |
|-----------|------|----------------|
| Float | Float | `/avatar/parameters/v2/JawOpen` |
| Bool | Bool | `/avatar/parameters/v2/JawOpen` |
| Binary bits | Bool | `/avatar/parameters/v2/JawOpen1`, `v2/JawOpen2`, ... |

The float and bool share the same address. If your avatar has a float parameter at `v2/JawOpen`, vrft_d sends float values. If it's typed as bool, it sends bool (threshold 0.5). Binary bits are discovered automatically if you name parameters with numeric suffixes (e.g., `v2/JawOpen1`, `v2/JawOpen2`).

vrft_d also automatically mirrors parameters to the `FT/` prefix — e.g., a match on `/avatar/parameters/v2/JawOpen` also sends to `/avatar/parameters/FT/v2/JawOpen`. You do not need both on your avatar.

## Eye & Gaze

| Parameter | Type | Range | Description |
|-----------|------|-------|-------------|
| `v2/EyeLeftX` | Float | -1 to 1 | Left eye horizontal gaze |
| `v2/EyeLeftY` | Float | -1 to 1 | Left eye vertical gaze |
| `v2/EyeRightX` | Float | -1 to 1 | Right eye horizontal gaze |
| `v2/EyeRightY` | Float | -1 to 1 | Right eye vertical gaze |
| `v2/EyeX` | Float | -1 to 1 | Combined eye horizontal gaze (average) |
| `v2/EyeY` | Float | -1 to 1 | Combined eye vertical gaze (average) |
| `v2/EyeOpenLeft` | Float | 0 to 1 | Left eye openness |
| `v2/EyeOpenRight` | Float | 0 to 1 | Right eye openness |
| `v2/EyeOpen` | Float | 0 to 1 | Combined eye openness |
| `v2/EyeClosedLeft` | Float | 0 to 1 | Left eye closure (inverse of open) |
| `v2/EyeClosedRight` | Float | 0 to 1 | Right eye closure |
| `v2/EyeClosed` | Float | 0 to 1 | Combined eye closure |
| `v2/EyeLidLeft` | Float | 0 to 1 | Left eyelid position |
| `v2/EyeLidRight` | Float | 0 to 1 | Right eyelid position |
| `v2/EyeLid` | Float | 0 to 1 | Combined eyelid |
| `v2/EyeWide` | Float | 0 to 1 | Eye widening (both eyes) |
| `v2/EyeSquint` | Float | 0 to 1 | Eye squint (right eye) |
| `v2/EyesSquint` | Float | 0 to 1 | Eye squint (both eyes combined) |
| `v2/PupilDilation` | Float | 0 to 1 | Pupil dilation (normalized) |
| `v2/PupilDiameterLeft` | Float | 0 to 1 | Left pupil diameter |
| `v2/PupilDiameterRight` | Float | 0 to 1 | Right pupil diameter |
| `v2/PupilDiameter` | Float | 0 to 1 | Combined pupil diameter |

## Brow

| Parameter | Type | Range | Description |
|-----------|------|-------|-------------|
| `v2/BrowUp` | Float | 0 to 1 | Combined brow raise |
| `v2/BrowUpLeft` | Float | 0 to 1 | Left brow raise |
| `v2/BrowUpRight` | Float | 0 to 1 | Right brow raise |
| `v2/BrowDown` | Float | 0 to 1 | Combined brow lower |
| `v2/BrowDownLeft` | Float | 0 to 1 | Left brow lower |
| `v2/BrowDownRight` | Float | 0 to 1 | Right brow lower |
| `v2/BrowInnerUp` | Float | 0 to 1 | Inner brow raise |
| `v2/BrowOuterUp` | Float | 0 to 1 | Outer brow raise |
| `v2/BrowExpression` | Float | -1 to 1 | Combined brow expression (down=-1, up=1) |
| `v2/BrowExpressionLeft` | Float | -1 to 1 | Left brow expression |
| `v2/BrowExpressionRight` | Float | -1 to 1 | Right brow expression |

## Jaw & Cheek

| Parameter | Type | Range | Description |
|-----------|------|-------|-------------|
| `v2/JawX` | Float | -1 to 1 | Jaw horizontal shift (left=-1, right=1) |
| `v2/JawZ` | Float | 0 to 1 | Jaw forward thrust |
| `v2/CheekSquint` | Float | 0 to 1 | Cheek squint (both) |
| `v2/CheekPuffSuck` | Float | -1 to 1 | Cheek puff/suck combined (puff=1, suck=-1) |
| `v2/CheekPuffSuckLeft` | Float | -1 to 1 | Left cheek puff/suck |
| `v2/CheekPuffSuckRight` | Float | -1 to 1 | Right cheek puff/suck |
| `v2/CheekSuck` | Float | 0 to 1 | Cheek suck |

## Mouth & Lips

| Parameter | Type | Range | Description |
|-----------|------|-------|-------------|
| `v2/MouthOpen` | Float | 0 to 1 | Mouth opening |
| `v2/MouthX` | Float | -1 to 1 | Mouth horizontal shift |
| `v2/MouthUpperX` | Float | -1 to 1 | Upper lip horizontal shift |
| `v2/MouthLowerX` | Float | -1 to 1 | Lower lip horizontal shift |
| `v2/MouthUpperUp` | Float | 0 to 1 | Upper lip raise |
| `v2/MouthLowerDown` | Float | 0 to 1 | Lower lip depression |
| `v2/MouthStretch` | Float | 0 to 1 | Lip stretch |
| `v2/MouthTightener` | Float | 0 to 1 | Lip tightener |
| `v2/MouthPress` | Float | 0 to 1 | Lips pressed together |
| `v2/MouthDimple` | Float | 0 to 1 | Mouth dimple |
| `v2/MouthSmileLeft` | Float | 0 to 1 | Left smile |
| `v2/MouthSmileRight` | Float | 0 to 1 | Right smile |
| `v2/MouthSadLeft` | Float | 0 to 1 | Left mouth sad |
| `v2/MouthSadRight` | Float | 0 to 1 | Right mouth sad |
| `v2/MouthCornerY` | Float | -1 to 1 | Mouth corner vertical (frown=-1, smile=1) |
| `v2/MouthCornerYLeft` | Float | -1 to 1 | Left mouth corner vertical |
| `v2/MouthCornerYRight` | Float | -1 to 1 | Right mouth corner vertical |
| `v2/SmileFrown` | Float | -1 to 1 | Smile/frown combined (frown=-1, smile=1) |
| `v2/SmileFrownLeft` | Float | -1 to 1 | Left smile/frown |
| `v2/SmileFrownRight` | Float | -1 to 1 | Right smile/frown |
| `v2/SmileSad` | Float | -1 to 1 | Smile/sad combined |
| `v2/SmileSadLeft` | Float | -1 to 1 | Left smile/sad |
| `v2/SmileSadRight` | Float | -1 to 1 | Right smile/sad |
| `v2/MouthTightenerStretch` | Float | -1 to 1 | Tightener/stretch combined |
| `v2/MouthTightenerStretchLeft` | Float | -1 to 1 | Left tightener/stretch |
| `v2/MouthTightenerStretchRight` | Float | -1 to 1 | Right tightener/stretch |
| `v2/NoseSneer` | Float | 0 to 1 | Nose sneer |
| `v2/LipSuckUpper` | Float | 0 to 1 | Upper lip suck |
| `v2/LipSuckLower` | Float | 0 to 1 | Lower lip suck |
| `v2/LipSuck` | Float | 0 to 1 | Combined lip suck |
| `v2/LipFunnelUpper` | Float | 0 to 1 | Upper lip funnel |
| `v2/LipFunnelLower` | Float | 0 to 1 | Lower lip funnel |
| `v2/LipFunnel` | Float | 0 to 1 | Combined lip funnel |
| `v2/LipPuckerUpper` | Float | 0 to 1 | Upper lip pucker |
| `v2/LipPuckerLower` | Float | 0 to 1 | Lower lip pucker |
| `v2/LipPuckerLeft` | Float | 0 to 1 | Left lip pucker |
| `v2/LipPuckerRight` | Float | 0 to 1 | Right lip pucker |
| `v2/LipPucker` | Float | 0 to 1 | Combined lip pucker |
| `v2/LipSuckFunnelUpper` | Float | -1 to 1 | Upper lip suck/funnel combined |
| `v2/LipSuckFunnelLower` | Float | -1 to 1 | Lower lip suck/funnel combined |
| `v2/LipSuckFunnelUpperLeft` | Float | -1 to 1 | Upper left lip suck/funnel |
| `v2/LipSuckFunnelUpperRight` | Float | -1 to 1 | Upper right lip suck/funnel |
| `v2/LipSuckFunnelLowerLeft` | Float | -1 to 1 | Lower left lip suck/funnel |
| `v2/LipSuckFunnelLowerRight` | Float | -1 to 1 | Lower right lip suck/funnel |

## Tongue

| Parameter | Type | Range | Description |
|-----------|------|-------|-------------|
| `v2/TongueX` | Float | -1 to 1 | Tongue horizontal position |
| `v2/TongueY` | Float | -1 to 1 | Tongue vertical position |
| `v2/TongueArchY` | Float | -1 to 1 | Tongue arch/shape vertical |
| `v2/TongueShape` | Float | -1 to 1 | Tongue shape blend |

## Head Pose

| Parameter | Type | Range | Description |
|-----------|------|-------|-------------|
| `v2/Head/Yaw` | Float | -1 to 1 | Head yaw (left/right rotation) |
| `v2/Head/Pitch` | Float | -1 to 1 | Head pitch (up/down rotation) |
| `v2/Head/Roll` | Float | -1 to 1 | Head roll (tilt) |
| `v2/Head/PosX` | Float | -1 to 1 | Head position X |
| `v2/Head/PosY` | Float | -1 to 1 | Head position Y |
| `v2/Head/PosZ` | Float | -1 to 1 | Head position Z |

## Legacy Parameters

Parameters without the `v2/` prefix (SRanipal-era names like `EyeLeftX`, `JawOpen`, `MouthSmile_L`) are supported for backwards compatibility with existing avatars. They are sent by the legacy eye and legacy lip parameter sets.

**Do not use legacy parameters for new avatars.** Use the `v2/` parameters above.
