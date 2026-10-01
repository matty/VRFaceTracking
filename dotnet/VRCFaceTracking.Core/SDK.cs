using System;
using System.Collections.Generic;
using System.IO;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Security.Principal;
using System.Text;

// Modules built against VRCFaceTracking 5.0 and 5.1 take these from Core;
// later ones take them from VRCFaceTracking.SDK, where they now live.
[assembly: TypeForwardedTo(typeof(VRCFaceTracking.Core.Library.ModuleState))]
[assembly: TypeForwardedTo(typeof(VRCFaceTracking.ModuleMetadata))]
[assembly: TypeForwardedTo(typeof(VRCFaceTracking.ExtTrackingModule))]

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

        /// <summary>The vector's x and y, dropping z.</summary>
        public static implicit operator Vector2(Vector3 v) => new Vector2(v.x, v.y);

        /// <summary>Negates x, in place, and returns the result.</summary>
        public Vector2 FlipXCoordinates()
        {
            x = -x;
            return this;
        }
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct Vector3
    {
        public float x;
        public float y;
        public float z;

        public Vector3(float x, float y, float z)
        {
            this.x = x;
            this.y = y;
            this.z = z;
        }

        public static Vector3 operator +(Vector3 a, Vector3 b) => new Vector3(a.x + b.x, a.y + b.y, a.z + b.z);
        public static Vector3 operator -(Vector3 a, Vector3 b) => new Vector3(a.x - b.x, a.y - b.y, a.z - b.z);
        public static Vector3 operator *(Vector3 a, float d) => new Vector3(a.x * d, a.y * d, a.z * d);
        public static Vector3 operator /(Vector3 a, float d) => new Vector3(a.x / d, a.y / d, a.z / d);
        public static Vector3 zero => new Vector3(0, 0, 0);

        /// <summary>Negates x, in place, and returns the result.</summary>
        public Vector3 FlipXCoordinates()
        {
            x = -x;
            return this;
        }
    }

    /// <summary>
    /// A camera image a module shares. Nothing here shows it; modules that
    /// fill one in still run.
    /// </summary>
    public class Image
    {
        public byte[]? ImageData;
        public (int x, int y) ImageSize;
        public bool SupportsImage;
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

namespace VRCFaceTracking.Core
{
    /// <summary>
    /// Helpers modules built against VRCFaceTracking 5.2 and later use.
    /// </summary>
    public static class Utils
    {
        /// <summary>Where modules keep their settings: VRCFaceTracking's own folder, so they find what they saved there.</summary>
        public static readonly string PersistentDataDirectory = VRCFaceTracking.Utils.PersistentDataDirectory;
    }
}

namespace VRCFaceTracking.Core.OSC
{
    /// <summary>
    /// One OSC message read from a packet, for modules that receive OSC
    /// themselves. Reading a bundle gives its messages one at a time.
    /// </summary>
    public class OscMessage
    {
        public string Address { get; private set; } = string.Empty;

        /// <summary>The first argument, or null when there is none.</summary>
        public object? Value { get; private set; }

        /// <summary>
        /// Reads the message at <paramref name="messageIndex"/> in the first
        /// <paramref name="len"/> bytes of <paramref name="bytes"/>, and moves
        /// the index past it. A packet that can't be read leaves an empty
        /// address and moves the index to the end, so a loop over a packet
        /// stops.
        /// </summary>
        public OscMessage(byte[] bytes, int len, ref int messageIndex)
        {
            len = Math.Min(len, bytes.Length);
            try
            {
                int i = messageIndex;
                if (Matches(bytes, len, i, "#bundle\0"))
                {
                    // "#bundle", then a time tag.
                    i += 16;
                }
                if (i < len && bytes[i] != (byte)'/')
                {
                    // A bundle element: its size, then the message.
                    i += 4;
                }
                Address = ReadString(bytes, len, ref i);
                if (i < len && bytes[i] == (byte)',')
                {
                    string tags = ReadString(bytes, len, ref i);
                    if (tags.Length > 1)
                    {
                        Value = ReadArgument(tags[1], bytes, len, ref i);
                    }
                    for (int t = 2; t < tags.Length; t++)
                    {
                        ReadArgument(tags[t], bytes, len, ref i);
                    }
                }
                messageIndex = Math.Min(i, len);
            }
            catch (Exception)
            {
                Address = string.Empty;
                Value = null;
                messageIndex = len;
            }
        }

        private static bool Matches(byte[] bytes, int len, int at, string text)
        {
            if (at + text.Length > len) return false;
            for (int k = 0; k < text.Length; k++)
            {
                if (bytes[at + k] != (byte)text[k]) return false;
            }
            return true;
        }

        private static int Padded(int length) => (length + 4) & ~3;

        private static string ReadString(byte[] bytes, int len, ref int i)
        {
            int end = Array.IndexOf(bytes, (byte)0, i, len - i);
            if (end < 0) throw new FormatException("unterminated OSC string");
            string text = Encoding.UTF8.GetString(bytes, i, end - i);
            i += Padded(end - i);
            return text;
        }

        private static int ReadInt(byte[] bytes, int len, ref int i)
        {
            if (i + 4 > len) throw new FormatException("truncated OSC argument");
            int value = System.Buffers.Binary.BinaryPrimitives.ReadInt32BigEndian(bytes.AsSpan(i, 4));
            i += 4;
            return value;
        }

        private static long ReadLong(byte[] bytes, int len, ref int i)
        {
            if (i + 8 > len) throw new FormatException("truncated OSC argument");
            long value = System.Buffers.Binary.BinaryPrimitives.ReadInt64BigEndian(bytes.AsSpan(i, 8));
            i += 8;
            return value;
        }

        private static object? ReadArgument(char tag, byte[] bytes, int len, ref int i)
        {
            switch (tag)
            {
                case 'i': return ReadInt(bytes, len, ref i);
                case 'f': return BitConverter.Int32BitsToSingle(ReadInt(bytes, len, ref i));
                case 'h': return ReadLong(bytes, len, ref i);
                case 'd': return BitConverter.Int64BitsToDouble(ReadLong(bytes, len, ref i));
                case 's': return ReadString(bytes, len, ref i);
                case 'b':
                {
                    int size = ReadInt(bytes, len, ref i);
                    if (size < 0 || i + size > len) throw new FormatException("truncated OSC blob");
                    byte[] blob = bytes.AsSpan(i, size).ToArray();
                    i += (size + 3) & ~3;
                    return blob;
                }
                case 'T': return true;
                case 'F': return false;
                case 'N':
                case 'I': return null;
                default: throw new FormatException($"unsupported OSC type tag '{tag}'");
            }
        }
    }
}

namespace VRCFaceTracking
{
    using VRCFaceTracking.Core.Params.Data;
    using VRCFaceTracking.Core.Types;

    /// <summary>
    /// Class that contains all relevant tracking data
    /// </summary>
    public class UnifiedTracking
    {
        public static UnifiedTrackingData Data = new UnifiedTrackingData();

        // Camera images some modules share. Nothing reads them here.
        public static Image EyeImageData = new Image();
        public static Image LipImageData = new Image();
    }

    /// <summary>
    /// Helpers modules built against VRCFaceTracking 5.0 and 5.1 use.
    /// </summary>
    public static class Utils
    {
        /// <summary>Where modules keep their settings: VRCFaceTracking's own folder, so they find what they saved there.</summary>
        public static readonly string PersistentDataDirectory = CreatePersistentDataDirectory();

        /// <summary>Whether this process runs as an administrator.</summary>
        public static readonly bool HasAdmin = IsAdmin();

        [DllImport("kernel32.dll")]
        public static extern IntPtr OpenProcess(int dwDesiredAccess, bool bInheritHandle, int dwProcessId);

        [DllImport("kernel32.dll")]
        public static extern bool ReadProcessMemory(int hProcess, IntPtr lpBaseAddress, byte[] lpBuffer, int dwSize, ref int lpNumberOfBytesRead);

        private static string CreatePersistentDataDirectory()
        {
            string path = Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
                "VRCFaceTracking");
            try
            {
                Directory.CreateDirectory(path);
            }
            catch (Exception)
            {
                // A module that writes there reports it; one that doesn't is unaffected.
            }
            return path;
        }

        private static bool IsAdmin()
        {
            if (!OperatingSystem.IsWindows()) return false;
            using var identity = WindowsIdentity.GetCurrent();
            return new WindowsPrincipal(identity).IsInRole(WindowsBuiltInRole.Administrator);
        }
    }
}
