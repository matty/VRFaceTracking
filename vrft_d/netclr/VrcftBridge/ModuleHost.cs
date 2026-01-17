using System.Runtime.InteropServices;
using System.Runtime.CompilerServices;
using VRCFaceTracking.Core;
using VRCFaceTracking.Core.Params.Expressions;
using VRCFaceTracking.Core.Types;

namespace VrcftBridge;

[StructLayout(LayoutKind.Sequential)]
public unsafe struct MarshaledTrackingData
{
    public float left_eye_gaze_x;
    public float left_eye_gaze_y;
    public float left_eye_gaze_z;
    public float left_eye_pupil_diameter_mm;
    public float left_eye_openness;

    public float right_eye_gaze_x;
    public float right_eye_gaze_y;
    public float right_eye_gaze_z;
    public float right_eye_pupil_diameter_mm;
    public float right_eye_openness;

    public float eye_max_dilation;
    public float eye_min_dilation;
    public float eye_left_diameter;
    public float eye_right_diameter;

    public float head_yaw;
    public float head_pitch;
    public float head_roll;
    public float head_pos_x;
    public float head_pos_y;
    public float head_pos_z;

    public fixed float shapes[200];
}

    // Matches LogLevel enum in Rust
    public enum LogLevel
    {
        Error = 1,
        Warn = 2,
        Info = 3,
        Debug = 4,
        Trace = 5,
    }

    public static unsafe class ModuleHost
    {
        private static ExtTrackingModule? _loadedModule;
        // Store the function pointer for logging
        private static delegate* unmanaged[Cdecl]<int, IntPtr, IntPtr, void> _loggerFn;

        [UnmanagedCallersOnly(EntryPoint = "LoadModule", CallConvs = new[] { typeof(CallConvCdecl) })]
        public static int LoadModule(IntPtr assemblyPathPtr, delegate* unmanaged[Cdecl]<int, IntPtr, IntPtr, void> logger)
        {
            _loggerFn = logger;
            try
            {
                var assemblyPath = Marshal.PtrToStringAnsi(assemblyPathPtr);
                if (string.IsNullOrEmpty(assemblyPath)) return -1;

                Log(LogLevel.Info, $"Attempting to load module from {assemblyPath}");

                var assembly = System.Reflection.Assembly.LoadFrom(assemblyPath);

                foreach (var type in assembly.GetExportedTypes())
                {
                    if (type.IsSubclassOf(typeof(ExtTrackingModule)) && !type.IsAbstract)
                    {
                        var module = Activator.CreateInstance(type) as ExtTrackingModule;
                        if (module != null)
                        {
                            _loadedModule = module;
                            
                            // Initialize module
                            try 
                            {
                                var (eye, expr) = module.Initialize(true, true);
                                Log(LogLevel.Info, $"Initialized module {type.Name}. Eye: {eye}, Expr: {expr}");
                            }
                            catch(Exception initEx)
                            {
                                Log(LogLevel.Error, $"Module {type.Name} threw during Initialize: {initEx}");
                                return -4;
                            }

                            return 0;
                        }
                    }
                }
                
                Log(LogLevel.Warn, $"No ExtTrackingModule found in {assemblyPath}");
                return -2;
            }
            catch (Exception ex)
            {
                Log(LogLevel.Error, $"Failed to load module: {ex}");
                return -3;
            }
        }

        private static void Log(LogLevel level, string message)
        {
            if (_loggerFn != null)
            {
                var target = Marshal.StringToHGlobalAnsi("VrcftBridge");
                var msg = Marshal.StringToHGlobalAnsi(message);
                _loggerFn((int)level, target, msg);
                Marshal.FreeHGlobal(target);
                Marshal.FreeHGlobal(msg);
            }
            else
            {
                Console.WriteLine($"[VrcftBridge/Fallback] {level}: {message}");
            }
        }

    [UnmanagedCallersOnly(EntryPoint = "Update", CallConvs = new[] { typeof(CallConvCdecl) })]
    public static unsafe void Update(MarshaledTrackingData* data)
    {
        if (_loadedModule == null) return;

        try
        {
            _loadedModule.Update();
            var src = UnifiedTracking.Data;

            // Marshal from UnifiedTracking.Data to our C-compatible struct
            data->left_eye_gaze_x = src.Eye.Left.Gaze.X;
            data->left_eye_gaze_y = src.Eye.Left.Gaze.Y;
            data->left_eye_gaze_z = 0; 
            
            data->left_eye_pupil_diameter_mm = src.Eye.Left.PupilDiameter_MM;
            data->left_eye_openness = src.Eye.Left.Openness;

            data->right_eye_gaze_x = src.Eye.Right.Gaze.X;
            data->right_eye_gaze_y = src.Eye.Right.Gaze.Y;
            data->right_eye_gaze_z = 0;

            data->right_eye_pupil_diameter_mm = src.Eye.Right.PupilDiameter_MM;
            data->right_eye_openness = src.Eye.Right.Openness;

            data->eye_max_dilation = src.Eye.MaxDilation;
            data->eye_min_dilation = src.Eye.MinDilation;
            
            data->eye_left_diameter = src.Eye.Left.PupilDiameter_MM;
            data->eye_right_diameter = src.Eye.Right.PupilDiameter_MM;

            data->head_yaw = src.Head.Yaw;
            data->head_pitch = src.Head.Pitch;
            data->head_roll = src.Head.Roll;
            data->head_pos_x = src.Head.X;
            data->head_pos_y = src.Head.Y;
            data->head_pos_z = src.Head.Z;

            // Copy shapes
            int maxShapes = Math.Min(200, src.Shapes.Length);
            for (int i = 0; i < maxShapes; i++)
            {
                data->shapes[i] = src.Shapes[i].Weight;
            }
        }
        catch (Exception e)
        {
            Console.WriteLine($"[VrcftBridge] Error in Update: {e}");
        }
    }

    [UnmanagedCallersOnly(EntryPoint = "Teardown", CallConvs = new[] { typeof(CallConvCdecl) })]
    public static void Teardown()
    {
        try
        {
            _loadedModule?.Teardown();
            _loadedModule = null;
        }
        catch (Exception ex)
        {
            Console.WriteLine($"[VrcftBridge] Error in Teardown: {ex}");
        }
    }
}
