using System;
using System.Collections.Generic;
using System.IO;
using Microsoft.Extensions.Logging;

// What modules built against VRCFaceTracking 5.2 and later take from
// VRCFaceTracking.SDK. VRCFaceTracking.Core forwards these types here, so
// older modules, which take them from Core, find them too.

namespace VRCFaceTracking.Core.Library
{
    public enum ModuleState
    {
        Uninitialized = -1,
        Idle = 0,
        Active = 1
    }
}

namespace VRCFaceTracking
{
    using VRCFaceTracking.Core.Library;

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
