using System.Buffers.Text;
using System.Security.Cryptography;
using System.Text;

namespace Comptroller.Service.Identity;

public static class Tokens
{
    // No 0/O, 1/I/L: codes get read aloud and retyped.
    private const string CodeAlphabet = "23456789ABCDEFGHJKMNPQRSTUVWXYZ";

    public static string NewToken() => Base64Url.EncodeToString(RandomNumberGenerator.GetBytes(32));

    public static string NewCode()
    {
        var chars = RandomNumberGenerator.GetString(CodeAlphabet, 12);
        return $"{chars[..4]}-{chars[4..8]}-{chars[8..]}";
    }

    public static string Hash(string value) =>
        Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(value)));

    public static string HashCode(string code) =>
        Hash(code.Replace("-", "").Replace(" ", "").ToUpperInvariant());
}
