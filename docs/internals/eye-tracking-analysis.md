# Eye Tracking Data Format Analysis

## Data Representation

The current implementation uses a `Vec2` for eye gaze tracking in `UnifiedSingleEyeData.gaze`. Different modules currently interpret this `Vec2` in two distinct ways:

1.  **Direction Components (Projections)**: Storing the X and Y components of a normalized 3D direction vector.
2.  **Angular Measurements**: Storing Yaw and Pitch as angles (typically in radians).

## Mathematical Context

### VRChat OSC Expectations

- `/tracking/eye/LeftRightVec`: Expects **Normalized 3D vectors** (Vec3).
- `/avatar/parameters/FT/v2/EyeLeftX/Y`: Expects **2D projected values**.

### Module Implementations

#### Component Projection (e.g., Virtual Desktop)

```rust
let left_gaze = left_quat * forward;  // 3D normalized vector
data.eye.left.gaze = Vec2::new(left_gaze.x, left_gaze.y);  // X,Y components
```

This approach stores the X and Y projections. For small angles where the gaze is mostly forward (z ≈ 1), these values closely approximate the angular values.

#### Angular Conversion (e.g., SRanipal)

```rust
data.eye.left.gaze = vector3_to_yaw_pitch(gaze);  // atan2(x,z), asin(y)
```

This approach converts the 3D vector into explicit yaw and pitch angles.

## Comparison of Approaches

| Feature         | Component Projection                     | Angular Conversion               |
| :-------------- | :--------------------------------------- | :------------------------------- |
| **Complexity**  | Low (Direct assignment)                  | Medium (Trigonometric functions) |
| **Accuracy**    | High (near center), Low (extreme angles) | High (all angles)                |
| **Consistency** | Matches projection-based OSC params      | Mathematically precise           |

## Future Considerations

To provide full 3D gaze information and ensure maximum compatibility with various tracking standards, transitioning `UnifiedSingleEyeData.gaze` from `Vec2` to `Vec3` is recommended. This would allow modules to pass the full normalized direction vector without loss of data or ambiguity in representation.
