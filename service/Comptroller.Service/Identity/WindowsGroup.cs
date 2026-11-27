using System.Runtime.Versioning;
using System.Security.Principal;

namespace Comptroller.Service.Identity;

public static class WindowsGroup
{
    /// <summary>Whether the signed-in Windows user is in the group, given by name or SID.</summary>
    [SupportedOSPlatform("windows")]
    public static bool Contains(WindowsIdentity identity, string group)
    {
        try
        {
            var sid = group.StartsWith("S-1-", StringComparison.OrdinalIgnoreCase)
                ? new SecurityIdentifier(group)
                : (SecurityIdentifier)new NTAccount(group).Translate(typeof(SecurityIdentifier));
            return new WindowsPrincipal(identity).IsInRole(sid);
        }
        catch (Exception ex) when (ex is IdentityNotMappedException or ArgumentException or SystemException)
        {
            Serilog.Log.Warning("Can't check the allowed group {Group}: {Error}", group, ex.Message);
            return false;
        }
    }
}
