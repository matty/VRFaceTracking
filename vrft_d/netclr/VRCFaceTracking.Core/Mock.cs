using System.Numerics;

namespace VRCFaceTracking.Core.Types
{
    public struct Vector2
    {
        public float X;
        public float Y;
    }

    public struct UnifiedSingleEyeData
    {
        public Vector2 Gaze;
        public float PupilDiameter_MM;
        public float Openness;
    }

    public struct UnifiedEyeData
    {
        public UnifiedSingleEyeData Left;
        public UnifiedSingleEyeData Right;
        public float MaxDilation;
        public float MinDilation;
    }

    public struct UnifiedExpressionShape
    {
        public float Weight;
    }

    public struct UnifiedHeadData
    {
        public float Yaw, Pitch, Roll;
        public float X, Y, Z;
    }

    public class UnifiedTrackingData
    {
        public UnifiedEyeData Eye;
        public UnifiedExpressionShape[] Shapes;
        public UnifiedHeadData Head;

        public UnifiedTrackingData()
        {
            Shapes = new UnifiedExpressionShape[200];
        }
    }
}

namespace VRCFaceTracking.Core
{
    using VRCFaceTracking.Core.Types;

    public static class UnifiedTracking
    {
        public static UnifiedTrackingData Data { get; } = new UnifiedTrackingData();
    }

    public abstract class ExtTrackingModule
    {
        public virtual (bool, bool) GetSupportedModuleStats() => (true, true);
        public abstract (bool eyeSuccess, bool expressionSuccess) Initialize(bool eyeAvailable, bool expressionAvailable);
        public abstract void Update();
        public abstract void Teardown();
    }
}

namespace VRCFaceTracking.Core.Params.Expressions
{
    public enum UnifiedExpressions
    {
        Max = 200
    }
}