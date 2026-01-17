using VRCFaceTracking.Core;
using VRCFaceTracking.Core.Params.Expressions;

namespace TestModule;

public class MyModule : ExtTrackingModule
{
    public override (bool, bool) GetSupportedModuleStats() => (true, true);

    public override void Update()
    {
        UnifiedTracking.Data.Eye.Left.Openness = 0.42f;
        UnifiedTracking.Data.Shapes[0].Weight = 0.99f;
    }

    public override void Teardown() {}
}
