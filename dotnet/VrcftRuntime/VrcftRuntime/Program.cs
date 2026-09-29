using System;
using System.IO;
using System.IO.MemoryMappedFiles;
using System.Reflection;
using System.Runtime.InteropServices;
using System.Threading;
using Microsoft.Extensions.Logging;

namespace VrcftRuntime;

[StructLayout(LayoutKind.Sequential, Pack = 1)]
public unsafe struct MarshaledTrackingData
{
    public float left_eye_gaze_x;
    public float left_eye_gaze_y;
    public float left_eye_pupil_diameter_mm;
    public float left_eye_openness;

    public float right_eye_gaze_x;
    public float right_eye_gaze_y;
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
    public ulong main_app_heartbeat;
    public ulong runtime_heartbeat;
}

class Program
{
    private static ILoggerFactory _loggerFactory;
    private static ILogger _logger;
    private static dynamic _module;
    private static dynamic _unifiedTracking;
    private static Assembly _sdkAssembly;
    
    private static MemoryMappedFile _mmf;
    private static MemoryMappedViewAccessor _accessor;

    // Cleared when the host shuts down, to stop the module's update thread.
    private static volatile bool _running = true;
    // When the module's Update() last returned, in Environment.TickCount64
    // milliseconds, so a long-blocking update can be logged.
    private static long _lastUpdateReturned = Environment.TickCount64;
    // Shortest time between the starts of two Update() calls, so a module
    // whose Update() returns at once doesn't spin a core. Matches how often
    // the data is copied to shared memory.
    private const int MinUpdateIntervalMs = 10;
    // How long Update() can go without returning before it's logged.
    private const int SlowUpdateWarningMs = 10_000;

    static void Main(string[] args)
    {
        LogLevel logLevel = LogLevel.Information;
        if (args.Length >= 2)
        {
            logLevel = args[1].ToLowerInvariant() switch
            {
                "trace" => LogLevel.Trace,
                "debug" => LogLevel.Debug,
                "info" => LogLevel.Information,
                "warn" => LogLevel.Warning,
                "error" => LogLevel.Error,
                _ => LogLevel.Information
            };
        }

        _loggerFactory = LoggerFactory.Create(builder => builder.AddConsole().SetMinimumLevel(logLevel));
        _logger = _loggerFactory.CreateLogger("ProxyHost");

        if (args.Length < 1)
        {
            _logger.LogError("Usage: VrcftRuntime.exe <module_path> [log_level]");
            return;
        }

        string modulePath = args[0];
        _logger.LogInformation("Attempting to load module from: {Path}", modulePath);

        try
        {
            LoadModule(modulePath);
            SetupSharedMemory();
            RunLoop();
        }
        catch (Exception ex)
        {
            _logger.LogCritical(ex, "Fatal error in ProxyHost");
        }
        finally
        {
            _running = false;
            try { _module?.Teardown(); } catch { }
            _accessor?.Dispose();
            _mmf?.Dispose();
        }
    }

    static void LoadModule(string path)
    {
        string absolutePath = Path.GetFullPath(path);
        _logger.LogInformation("Loading assembly from: {Path}", absolutePath);

        var loadContext = new ModuleLoadContext(absolutePath, _logger);
        var assembly = loadContext.LoadFromAssemblyPath(absolutePath);
        _logger.LogInformation("Assembly Loaded: {Name}", assembly.FullName);

        // Find ExtTrackingModule type dynamically (don't reference our embedded SDK)
        Type extTrackingModuleType = null;
        
        foreach (var type in assembly.GetExportedTypes())
        {
            _logger.LogDebug("Checking type: {FullName}. BaseType: {BaseType}", type.FullName, type.BaseType?.FullName);
            
            // Check by name to avoid type identity issues
            if (type.BaseType?.Name == "ExtTrackingModule")
            {
                if (type.IsAbstract) continue;

                _logger.LogInformation("Found module type: {Type}", type.FullName);
                extTrackingModuleType = type.BaseType;
                
                // Create instance using dynamic
                _module = Activator.CreateInstance(type);
                
                // Set Logger field using reflection
                var loggerField = extTrackingModuleType.GetField("Logger");
                if (loggerField != null)
                {
                    loggerField.SetValue(_module, _loggerFactory.CreateLogger(type.Name));
                }
                
                // Initialize using dynamic dispatch
                var result = _module.Initialize(true, true);
                bool eyeSuccess = result.Item1;
                bool exprSuccess = result.Item2;
                _logger.LogInformation("Initialized {Module}. Eye: {Eye}, Expr: {Expr}", type.Name, eyeSuccess, exprSuccess);
                
                _sdkAssembly = extTrackingModuleType.Assembly;
                var unifiedTrackingType = _sdkAssembly.GetType("VRCFaceTracking.UnifiedTracking");
                if (unifiedTrackingType != null)
                {
                    var dataField = unifiedTrackingType.GetField("Data", BindingFlags.Public | BindingFlags.Static);
                    if (dataField != null)
                    {
                        _unifiedTracking = dataField.GetValue(null);
                        _logger.LogInformation("Successfully bound to UnifiedTracking.Data from module's SDK context");
                    }
                }
                
                if (_unifiedTracking == null)
                {
                    throw new Exception("Failed to get UnifiedTracking.Data from SDK assembly");
                }
                
                // Set Status to Active if initialization succeeded
                if (eyeSuccess || exprSuccess)
                {
                    var statusField = extTrackingModuleType.GetField("Status");
                    if (statusField != null)
                    {
                        // Get ModuleState.Active from the SDK's ModuleState enum
                        var moduleStateType = _sdkAssembly.GetType("VRCFaceTracking.Core.Library.ModuleState");
                        if (moduleStateType != null)
                        {
                            var activeValue = Enum.Parse(moduleStateType, "Active");
                            statusField.SetValue(_module, activeValue);
                            _logger.LogInformation("Module Status set to Active");
                        }
                    }
                }
                
                return;
            }
        }
        throw new Exception("No valid ExtTrackingModule found in assembly. Checked " + assembly.GetExportedTypes().Length + " exported types.");
    }

    private class ModuleLoadContext : System.Runtime.Loader.AssemblyLoadContext
    {
        private readonly System.Runtime.Loader.AssemblyDependencyResolver _resolver;
        private readonly ILogger _logger;
        // The module's own folder. Registry modules rarely ship a .deps.json,
        // so the resolver alone finds nothing; their dependencies sit beside
        // them or in subfolders such as ModuleLibs/.
        private readonly string _moduleDir;

        public ModuleLoadContext(string mainAssemblyPath, ILogger logger) : base("ModuleContext", isCollectible: true)
        {
            _resolver = new System.Runtime.Loader.AssemblyDependencyResolver(mainAssemblyPath);
            _logger = logger;
            _moduleDir = Path.GetDirectoryName(mainAssemblyPath) ?? ".";
        }

        /// <summary>The first file named <paramref name="fileName"/> in the module's folder, then its subfolders.</summary>
        private string? FindInModule(string fileName)
        {
            string direct = Path.Combine(_moduleDir, fileName);
            if (File.Exists(direct)) return direct;
            try
            {
                return Directory.EnumerateFiles(_moduleDir, fileName, SearchOption.AllDirectories).FirstOrDefault();
            }
            catch (Exception ex)
            {
                _logger.LogDebug(ex, "Couldn't search {Dir} for {File}", _moduleDir, fileName);
                return null;
            }
        }

        protected override IntPtr LoadUnmanagedDll(string unmanagedDllName)
        {
            string? path = _resolver.ResolveUnmanagedDllToPath(unmanagedDllName);
            if (path == null)
            {
                string fileName = unmanagedDllName.EndsWith(".dll", StringComparison.OrdinalIgnoreCase)
                    ? unmanagedDllName
                    : unmanagedDllName + ".dll";
                path = FindInModule(Path.GetFileName(fileName));
            }
            if (path == null) return IntPtr.Zero;
            _logger.LogDebug("ALC Resolved native: {Name} -> {Path}", unmanagedDllName, path);
            return LoadUnmanagedDllFromPath(path);
        }

        protected override Assembly Load(AssemblyName assemblyName)
        {
            // System, Microsoft, and VRCFaceTracking.Core assemblies fallback to the default context (provided by the host)
            if (assemblyName.Name.StartsWith("System.") || assemblyName.Name == "netstandard" || assemblyName.Name.StartsWith("Microsoft.") || assemblyName.Name == "VRCFaceTracking.Core")
            {
                return null;
            }

            // Everything else (including VRCFaceTracking.Core) loads from module directory
            string assemblyPath = _resolver.ResolveAssemblyToPath(assemblyName);
            if (assemblyPath != null)
            {
                _logger.LogDebug("ALC Resolved: {Name} -> {Path}", assemblyName.Name, assemblyPath);
                return LoadFromAssemblyPath(assemblyPath);
            }

            string? besideModule = FindInModule(assemblyName.Name + ".dll");
            if (besideModule != null)
            {
                _logger.LogDebug("ALC Found: {Name} -> {Path}", assemblyName.Name, besideModule);
                return LoadFromAssemblyPath(besideModule);
            }

            return null;
        }
    }

    static unsafe void SetupSharedMemory()
    {
        _mmf = MemoryMappedFile.CreateOrOpen(@"Local\VRCFT_TrackingData", sizeof(MarshaledTrackingData));
        _accessor = _mmf.CreateViewAccessor();
        _logger.LogInformation(@"Shared memory setup complete: Local\VRCFT_TrackingData");
    }

    /// <summary>
    /// Calls the module's Update() over and over on its own thread, as VRCFT
    /// does. Modules may block in Update() while they wait for their device,
    /// sometimes for seconds, so it mustn't hold up the heartbeat the daemon
    /// watches to tell whether this host is still alive.
    /// </summary>
    static void StartUpdateThread()
    {
        var thread = new Thread(() =>
        {
            while (_running)
            {
                long started = Environment.TickCount64;
                try
                {
                    _module.Update();
                }
                catch (Exception ex)
                {
                    _logger.LogError(ex, "Error in module update");
                    Thread.Sleep(1000);
                }
                long now = Environment.TickCount64;
                Interlocked.Exchange(ref _lastUpdateReturned, now);
                int remaining = MinUpdateIntervalMs - (int)Math.Min(now - started, MinUpdateIntervalMs);
                if (remaining > 0) Thread.Sleep(remaining);
            }
        })
        {
            IsBackground = true,
            Name = "Module update",
        };
        thread.Start();
    }

    static unsafe void RunLoop()
    {
        _logger.LogInformation("Entering update loop...");
        StartUpdateThread();
        var data = new MarshaledTrackingData();
        ulong lastMainAppHeartbeat = 0;
        DateTime lastMainAppUpdate = DateTime.UtcNow;
        bool slowUpdateLogged = false;

        while (true)
        {
            try
            {
                long sinceUpdate = Environment.TickCount64 - Interlocked.Read(ref _lastUpdateReturned);
                if (sinceUpdate > SlowUpdateWarningMs && !slowUpdateLogged)
                {
                    _logger.LogWarning(
                        "The module's Update() hasn't returned for {Seconds} s. It may be waiting for its device; the last data is kept until it does.",
                        sinceUpdate / 1000);
                    slowUpdateLogged = true;
                }
                else if (sinceUpdate <= SlowUpdateWarningMs && slowUpdateLogged)
                {
                    _logger.LogInformation("The module's Update() is returning again.");
                    slowUpdateLogged = false;
                }

                // Copies what the module last wrote. Update() writes it on its
                // own thread, as it does in VRCFT, which reads it the same way.
                // Access UnifiedTracking.Data dynamically
                dynamic src = _unifiedTracking;
                dynamic eye = src.Eye;
                dynamic head = src.Head;
                dynamic shapes = src.Shapes;

                // Sync eye data
                dynamic leftEye = eye.Left;
                dynamic rightEye = eye.Right;
                dynamic leftGaze = leftEye.Gaze;
                dynamic rightGaze = rightEye.Gaze;

                data.left_eye_gaze_x = (float)leftGaze.x;
                data.left_eye_gaze_y = (float)leftGaze.y;
                data.left_eye_pupil_diameter_mm = (float)leftEye.PupilDiameter_MM;
                data.left_eye_openness = (float)leftEye.Openness;

                data.right_eye_gaze_x = (float)rightGaze.x;
                data.right_eye_gaze_y = (float)rightGaze.y;
                data.right_eye_pupil_diameter_mm = (float)rightEye.PupilDiameter_MM;
                data.right_eye_openness = (float)rightEye.Openness;

                data.eye_max_dilation = (float)eye._maxDilation;
                data.eye_min_dilation = (float)eye._minDilation;

                data.head_yaw = (float)head.HeadYaw;
                data.head_pitch = (float)head.HeadPitch;
                data.head_roll = (float)head.HeadRoll;
                data.head_pos_x = (float)head.HeadPosX;
                data.head_pos_y = (float)head.HeadPosY;
                data.head_pos_z = (float)head.HeadPosZ;

                // Sync shapes
                int count = Math.Min(200, shapes.Length);
                for (int i = 0; i < count; i++)
                {
                    data.shapes[i] = (float)shapes[i].Weight;
                }

                // Update runtime heartbeat
                data.runtime_heartbeat++;

                // Read current main_app_heartbeat from shared memory BEFORE writing,
                // so we don't overwrite it with a stale value
                MarshaledTrackingData currentShmem;
                _accessor.Read(0, out currentShmem);
                data.main_app_heartbeat = currentShmem.main_app_heartbeat;

                // Write to shared memory (preserving main_app_heartbeat)
                _accessor.Write(0, ref data);

                // Check main app heartbeat for timeout detection
                if (data.main_app_heartbeat != lastMainAppHeartbeat)
                {
                    lastMainAppHeartbeat = data.main_app_heartbeat;
                    lastMainAppUpdate = DateTime.UtcNow;
                }
                else if (lastMainAppHeartbeat > 0 && (DateTime.UtcNow - lastMainAppUpdate).TotalSeconds > 10)
                {
                    // Only check timeout if we've ever seen a heartbeat (lastMainAppHeartbeat > 0)
                    _logger.LogWarning("Main app heartbeat lost. Shutting down.");
                    break;
                }
                
                Thread.Sleep(10);
            }
            catch (Exception ex)
            {
                _logger.LogError(ex, "Error in update loop");
                Thread.Sleep(1000);
            }
        }
    }
}
