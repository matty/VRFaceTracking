using System;
using System.Collections.Generic;
using System.IO;
using System.Runtime.InteropServices;
using Microsoft.Extensions.Logging;

namespace VRCFaceTracking.Core.Library
{
    public enum ModuleState
    {
        Uninitialized = -1,
        Idle = 0,
        Active = 1
    }
}

namespace VRCFaceTracking.Core.Types
{
    [StructLayout(LayoutKind.Sequential)]
    public struct Vector2
    {
        public float x;
        public float y;

        public Vector2(float x, float y)
        {
            this.x = x;
            this.y = y;
        }

        public static Vector2 operator +(Vector2 a, Vector2 b) => new Vector2(a.x + b.x, a.y + b.y);
        public static Vector2 operator /(Vector2 a, float d) => new Vector2(a.x / d, a.y / d);
        public static Vector2 operator *(Vector2 a, float d) => new Vector2(a.x * d, a.y * d);
        public static Vector2 operator -(Vector2 a, Vector2 b) => new Vector2(a.x - b.x, a.y - b.y);
        public static Vector2 zero => new Vector2(0, 0);
    }
}

namespace VRCFaceTracking.Core.Params.Expressions
{
    public enum UnifiedExpressions
    {
        // Eye Expressions
        EyeSquintRight = 0,
        EyeSquintLeft,
        EyeWideRight,
        EyeWideLeft,

        // Eyebrow Expressions
        BrowPinchRight,
        BrowPinchLeft,
        BrowLowererRight,
        BrowLowererLeft,
        BrowInnerUpRight,
        BrowInnerUpLeft,
        BrowOuterUpRight,
        BrowOuterUpLeft,

        // Nose Expressions
        NasalDilationRight,
        NasalDilationLeft,
        NasalConstrictRight,
        NasalConstrictLeft,

        // Cheek Expressions
        CheekSquintRight,
        CheekSquintLeft,
        CheekPuffRight,
        CheekPuffLeft,
        CheekSuckRight,
        CheekSuckLeft,

        // Jaw Exclusive Expressions
        JawOpen,
        JawRight,
        JawLeft,
        JawForward,
        JawBackward,
        JawClench,
        JawMandibleRaise,
        MouthClosed,

        // Lip Expressions
        LipSuckUpperRight,
        LipSuckUpperLeft,
        LipSuckLowerRight,
        LipSuckLowerLeft,
        LipSuckCornerRight,
        LipSuckCornerLeft,
        LipFunnelUpperRight,
        LipFunnelUpperLeft,
        LipFunnelLowerRight,
        LipFunnelLowerLeft,
        LipPuckerUpperRight,
        LipPuckerUpperLeft,
        LipPuckerLowerRight,
        LipPuckerLowerLeft,

        // Upper lip raiser group
        MouthUpperUpRight,
        MouthUpperUpLeft,
        MouthUpperDeepenRight,
        MouthUpperDeepenLeft,
        NoseSneerRight,
        NoseSneerLeft,

        // Lower lip depressor group
        MouthLowerDownRight,
        MouthLowerDownLeft,

        // Mouth Direction group
        MouthUpperRight,
        MouthUpperLeft,
        MouthLowerRight,
        MouthLowerLeft,

        // Smile group
        MouthCornerPullRight,
        MouthCornerPullLeft,
        MouthCornerSlantRight,
        MouthCornerSlantLeft,

        // Sad group
        MouthFrownRight,
        MouthFrownLeft,
        MouthStretchRight,
        MouthStretchLeft,
        MouthDimpleRight,
        MouthDimpleLeft,
        MouthRaiserUpper,
        MouthRaiserLower,
        MouthPressRight,
        MouthPressLeft,
        MouthTightenerRight,
        MouthTightenerLeft,

        // Tongue Expressions
        TongueOut,
        TongueUp,
        TongueDown,
        TongueRight,
        TongueLeft,
        TongueRoll,
        TongueBendDown,
        TongueCurlUp,
        TongueSquish,
        TongueFlat,
        TongueTwistRight,
        TongueTwistLeft,

        // Throat/Neck Expressions
        SoftPalateClose,
        ThroatSwallow,
        NeckFlexRight,
        NeckFlexLeft,

        Max
    }
}

namespace VRCFaceTracking.Core.Params.Data
{
    using VRCFaceTracking.Core.Types;
    using VRCFaceTracking.Core.Params.Expressions;

    /// <summary>
    /// Struct that represents a single eye.
    /// </summary>
    public struct UnifiedSingleEyeData
    {
        public Vector2 Gaze;
        public float PupilDiameter_MM;
        public float Openness;

        public UnifiedSingleEyeData()
        {
            Openness = 1.0f;
        }
    }

    /// <summary>
    /// Class that represents all possible eye data.
    /// </summary>
    public class UnifiedEyeData
    {
        public UnifiedSingleEyeData Left = new UnifiedSingleEyeData();
        public UnifiedSingleEyeData Right = new UnifiedSingleEyeData();
        public float _maxDilation;
        public float _minDilation = 999f;
        public float _leftDiameter;
        public float _rightDiameter;
    }

    /// <summary>
    /// Container of information pertaining to a singular Unified Expression shape.
    /// </summary>
    public struct UnifiedExpressionShape
    {
        public float Weight;
    }

    /// <summary>
    /// Head pose data container.
    /// </summary>
    public struct UnifiedHeadData
    {
        public float HeadYaw;
        public float HeadPitch;
        public float HeadRoll;
        public float HeadPosX;
        public float HeadPosY;
        public float HeadPosZ;
    }

    /// <summary>
    /// All data that is accessible by modules and is output to parameters.
    /// </summary>
    public class UnifiedTrackingData
    {
        public UnifiedEyeData Eye = new UnifiedEyeData();
        public UnifiedExpressionShape[] Shapes = new UnifiedExpressionShape[(int)UnifiedExpressions.Max + 1];
        public UnifiedHeadData Head = new UnifiedHeadData();
    }
}

namespace VRCFaceTracking
{
    using VRCFaceTracking.Core.Library;
    using VRCFaceTracking.Core.Params.Data;

    /// <summary>
    /// Module metadata structure
    /// </summary>
    public struct ModuleMetadata
    {
        public delegate void ActiveChange(bool state);
        public ActiveChange OnActiveChange;

        public List<Stream> StaticImages { get; set; }
        public string Name { get; set; }
        private bool _active;

        public bool Active
        {
            get => _active;
            set
            {
                _active = value;
                OnActiveChange?.Invoke(value);
            }
        }

        private bool _usingEye;
        private bool _usingExpression;

        public bool UsingEye
        {
            get => _usingEye;
            set => _usingEye = value;
        }

        public bool UsingExpression
        {
            get => _usingExpression;
            set => _usingExpression = value;
        }
    }

    /// <summary>
    /// Class that contains all relevant tracking data
    /// </summary>
    public class UnifiedTracking
    {
        public static UnifiedTrackingData Data = new UnifiedTrackingData();
    }

    /// <summary>
    /// Abstract base class for tracking modules
    /// </summary>
    public abstract class ExtTrackingModule
    {
        public virtual (bool SupportsEye, bool SupportsExpression) Supported => (false, false);

        public ModuleState Status = ModuleState.Uninitialized;

        public ILogger Logger;

        public ModuleMetadata ModuleInformation;

        public abstract (bool eyeSuccess, bool expressionSuccess) Initialize(bool eyeAvailable, bool expressionAvailable);
        public abstract void Update();
        public abstract void Teardown();
    }
}
