use api::UnifiedTrackingData;
use glam::Vec3;

const SHAPE_COUNT: usize = 200;

#[repr(C)]
pub struct MarshaledTrackingData {
    pub left_eye_gaze_x: f32,
    pub left_eye_gaze_y: f32,
    pub left_eye_gaze_z: f32,
    pub left_eye_pupil_diameter_mm: f32,
    pub left_eye_openness: f32,

    pub right_eye_gaze_x: f32,
    pub right_eye_gaze_y: f32,
    pub right_eye_gaze_z: f32,
    pub right_eye_pupil_diameter_mm: f32,
    pub right_eye_openness: f32,

    pub eye_max_dilation: f32,
    pub eye_min_dilation: f32,
    pub eye_left_diameter: f32,
    pub eye_right_diameter: f32,

    pub head_yaw: f32,
    pub head_pitch: f32,
    pub head_roll: f32,
    pub head_pos_x: f32,
    pub head_pos_y: f32,
    pub head_pos_z: f32,

    pub shapes: [f32; SHAPE_COUNT],
}

impl From<&MarshaledTrackingData> for UnifiedTrackingData {
    fn from(m: &MarshaledTrackingData) -> Self {
        let mut data = UnifiedTrackingData::default();
        
        // Left Eye
        data.eye.left.gaze = Vec3::new(m.left_eye_gaze_x, m.left_eye_gaze_y, m.left_eye_gaze_z);
        data.eye.left.pupil_diameter_mm = m.left_eye_pupil_diameter_mm;
        data.eye.left.openness = m.left_eye_openness;

        // Right Eye
        data.eye.right.gaze = Vec3::new(m.right_eye_gaze_x, m.right_eye_gaze_y, m.right_eye_gaze_z);
        data.eye.right.pupil_diameter_mm = m.right_eye_pupil_diameter_mm;
        data.eye.right.openness = m.right_eye_openness;
        
        // Eye General
        data.eye.max_dilation = m.eye_max_dilation;
        data.eye.min_dilation = m.eye_min_dilation;
        data.eye.left_diameter = m.eye_left_diameter;
        data.eye.right_diameter = m.eye_right_diameter;
        
        // Head
        data.head.head_yaw = m.head_yaw;
        data.head.head_pitch = m.head_pitch;
        data.head.head_roll = m.head_roll;
        data.head.head_pos_x = m.head_pos_x;
        data.head.head_pos_y = m.head_pos_y;
        data.head.head_pos_z = m.head_pos_z;

        // Shapes
        for (i, weight) in m.shapes.iter().enumerate() {
            if i < data.shapes.len() {
                data.shapes[i].weight = *weight;
            }
        }
        
        data
    }
}

impl From<&UnifiedTrackingData> for MarshaledTrackingData {
    fn from(d: &UnifiedTrackingData) -> Self {
        let mut shapes = [0.0; SHAPE_COUNT];
        for (i, shape) in d.shapes.iter().enumerate() {
            if i < SHAPE_COUNT {
                shapes[i] = shape.weight;
            }
        }
        
        Self {
            left_eye_gaze_x: d.eye.left.gaze.x,
            left_eye_gaze_y: d.eye.left.gaze.y,
            left_eye_gaze_z: d.eye.left.gaze.z,
            left_eye_pupil_diameter_mm: d.eye.left.pupil_diameter_mm,
            left_eye_openness: d.eye.left.openness,
            
            right_eye_gaze_x: d.eye.right.gaze.x,
            right_eye_gaze_y: d.eye.right.gaze.y,
            right_eye_gaze_z: d.eye.right.gaze.z,
            right_eye_pupil_diameter_mm: d.eye.right.pupil_diameter_mm,
            right_eye_openness: d.eye.right.openness,
            
            eye_max_dilation: d.eye.max_dilation,
            eye_min_dilation: d.eye.min_dilation,
            eye_left_diameter: d.eye.left_diameter,
            eye_right_diameter: d.eye.right_diameter,
            
            head_yaw: d.head.head_yaw,
            head_pitch: d.head.head_pitch,
            head_roll: d.head.head_roll,
            head_pos_x: d.head.head_pos_x,
            head_pos_y: d.head.head_pos_y,
            head_pos_z: d.head.head_pos_z,
            
            shapes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use api::{UnifiedTrackingData, UnifiedExpressions};

    #[test]
    fn test_round_trip() {
        let mut original = UnifiedTrackingData::default();
        original.eye.left.gaze.x = 0.5;
        // Check bounds before indexing
        let jaw_open_idx = UnifiedExpressions::JawOpen as usize;
        if jaw_open_idx < original.shapes.len() {
             original.shapes[jaw_open_idx].weight = 0.8;
        }

        let marshaled = MarshaledTrackingData::from(&original);
        
        assert_eq!(marshaled.left_eye_gaze_x, 0.5);
        if jaw_open_idx < SHAPE_COUNT {
            assert_eq!(marshaled.shapes[jaw_open_idx], 0.8);
        }

        let result = UnifiedTrackingData::from(&marshaled);
        
        assert_eq!(result.eye.left.gaze.x, 0.5);
        if jaw_open_idx < result.shapes.len() {
            assert_eq!(result.shapes[jaw_open_idx].weight, 0.8);
        }
    }
}