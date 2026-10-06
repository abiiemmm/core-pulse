using System.Globalization;
using System.Reflection;
using System.Security.Principal;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using LibreHardwareMonitor.Hardware;

namespace CorePulse.SensorHost;

internal static class Program
{
    const string Provider = "LibreHardwareMonitor 0.9.6";
    static readonly JsonSerializerOptions Json = new() { PropertyNamingPolicy = JsonNamingPolicy.SnakeCaseLower };

    public static int Main(string[] args)
    {
        CultureInfo.CurrentCulture = CultureInfo.InvariantCulture;
        CultureInfo.CurrentUICulture = CultureInfo.InvariantCulture;
        Console.OutputEncoding = new UTF8Encoding(false);
        if (args.SequenceEqual(new[] { "--self-test" })) return SelfTest();
        if (!args.SequenceEqual(new[] { "--stdio" })) return 2;
        // Library diagnostics cannot contaminate the framed protocol on stdout.
        using var output = new StreamWriter(Console.OpenStandardOutput(), new UTF8Encoding(false)) { AutoFlush = true };
        Console.SetOut(TextWriter.Null);
        var computer = new Computer(new ReadOnlySettings()) { IsCpuEnabled = true, IsGpuEnabled = true };
        try
        {
            computer.Open();
            ulong sequence = 0;
            while (Console.ReadLine() is { } request)
            {
                if (request == "quit") break;
                if (request != "sample") return 2;
                var devices = computer.Hardware.Where(h => h.HardwareType is HardwareType.Cpu or HardwareType.GpuNvidia or HardwareType.GpuAmd or HardwareType.GpuIntel)
                    .OrderBy(h => h.Identifier.ToString(), StringComparer.Ordinal).Take(32).Select(ReadDevice).ToArray();
                var packet = new Packet(1, "snapshot", ++sequence, DateTimeOffset.UtcNow.ToString("O"), Provider, IsElevated(), devices);
                var line = JsonSerializer.Serialize(packet, Json);
                if (Encoding.UTF8.GetByteCount(line) > 262144) return 3;
                output.WriteLine(line);
            }
            return 0;
        }
        catch
        {
            Console.Error.WriteLine("Sensor provider failed; basic monitoring remains available.");
            return 1;
        }
        finally { computer.Close(); }
    }

    static Device ReadDevice(IHardware hardware)
    {
        var kind = hardware.HardwareType == HardwareType.Cpu ? "cpu" : "gpu";
        var fresh = new HashSet<string>(StringComparer.Ordinal);
        var before = new HashSet<string>(StringComparer.Ordinal);
        var failed = false;
        // LHM can retain the last value when a native driver call fails. Clear
        // only in-memory load/temperature readings before every update so the
        // protocol never presents retained values as a new measurement.
        try
        {
            foreach (var part in Parts(hardware))
            {
                foreach (var sensor in part.Sensors.Where(Allowed))
                {
                    before.Add(ReadingId(sensor));
                    sensor.ValuesTimeWindow = TimeSpan.Zero;
                    if (InvalidateReading(sensor)) fresh.Add(ReadingId(sensor));
                }
                part.Update();
            }
        }
        catch { failed = true; }
        var readings = Parts(hardware).SelectMany(part => part.Sensors).Where(Allowed).Take(128).Select(sensor =>
            new Reading(ReadingId(sensor), sensor.Name, sensor.SensorType == SensorType.Temperature ? "temperature" : "load",
                sensor.SensorType == SensorType.Temperature ? "°C" : "%",
                // Newly activated sensors were produced by this update. Existing
                // readings must have been invalidated successfully beforehand.
                // LHM's PawnIO Execute returns a zero-filled buffer on driver
                // failure. CPU temperatures are withheld until per-call driver
                // success can be verified, even if the library reports zero.
                !failed && !(kind == "cpu" && sensor.SensorType == SensorType.Temperature) && (fresh.Contains(ReadingId(sensor)) || !before.Contains(ReadingId(sensor))) ? ValidValue(sensor.Value, sensor.SensorType == SensorType.Temperature) : null)).ToArray();
        return new Device(hardware.Identifier.ToString(), hardware.Name, kind, failed ? "error" : "ready", readings);
    }
    static IEnumerable<IHardware> Parts(IHardware hardware)
    {
        yield return hardware;
        foreach (var child in hardware.SubHardware) foreach (var part in Parts(child)) yield return part;
    }
    static bool Allowed(ISensor sensor) => sensor.SensorType is SensorType.Temperature or SensorType.Load;
    // Some upstream sensors share an Identifier (e.g. NVIDIA bus/memory load).
    // The fixed hash of the name disambiguates them without unstable ordinals.
    internal static string ReadingId(ISensor sensor) => sensor.Identifier + "/" + Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(sensor.Name)))[..16].ToLowerInvariant();
    internal static bool InvalidateReading(object sensor)
    {
        var property = sensor.GetType().GetProperty("Value", BindingFlags.Instance | BindingFlags.Public);
        if (property?.PropertyType != typeof(float?) || !property.CanWrite) return false;
        property.SetValue(sensor, null);
        return true;
    }
    internal static double? ValidValue(float? value, bool temperature)
        => value is { } number && float.IsFinite(number) && number >= 0 && number <= (temperature ? 150 : 100) ? number : null;
    static bool IsElevated()
    {
        using var identity = WindowsIdentity.GetCurrent();
        return new WindowsPrincipal(identity).IsInRole(WindowsBuiltInRole.Administrator);
    }
    static int SelfTest()
    {
        // Repeated permission checks must not leak an access-token handle.
        IsElevated();
        using var process = System.Diagnostics.Process.GetCurrentProcess();
        var handlesBefore = process.HandleCount;
        for (var i = 0; i < 250; i++) IsElevated();
        process.Refresh();
        if (process.HandleCount - handlesBefore > 4) return 1;
        var retained = new FakeReading { Value = 73 };
        if (!InvalidateReading(retained) || retained.Value != null || ValidValue(0, false) != 0 || ValidValue(float.NaN, true) != null || ValidValue(151, true) != null || ValidValue(null, true) != null) return 1;
        var settings = new ReadOnlySettings();
        settings.SetValue("/gpu/control/mode", "1");
        if (settings.GetValue("/gpu/control/mode", "0") != "0") return 1;
        var json = JsonSerializer.Serialize(new Packet(1, "snapshot", 1, "2026-10-06T00:00:00Z", Provider, false,
            [new Device("/gpu/0", "GPU Español", "gpu", "ready", [new Reading("/gpu/0/load/0", "GPU Core", "load", "%", 0), new Reading("/gpu/0/temperature/0", "GPU Core", "temperature", "°C", null)])]), Json);
        using var parsed = JsonDocument.Parse(json);
        if (parsed.RootElement.GetProperty("devices")[0].GetProperty("readings")[0].GetProperty("value").GetDouble() != 0 || parsed.RootElement.GetProperty("devices")[0].GetProperty("readings")[1].GetProperty("value").ValueKind != JsonValueKind.Null) return 1;
        Console.WriteLine("Token handle lifetime, sensor freshness, null/zero, bounds, read-only settings, UTF-8 protocol: passed.");
        return 0;
    }
    sealed class FakeReading { public float? Value { get; set; } }
}

// No file-backed settings and no imported fan-control modes. Undefined control
// mode stays at the library default; the host never calls control setters.
internal sealed class ReadOnlySettings : ISettings
{
    public bool Contains(string name) => false;
    public string GetValue(string name, string value) => value;
    public void SetValue(string name, string value) { }
    public void Remove(string name) { }
}
internal record Packet(int ProtocolVersion, string Type, ulong Sequence, string RecordedAt, string Provider, bool Elevated, Device[] Devices);
internal record Device(string Id, string Name, string Kind, string Status, Reading[] Readings);
internal record Reading(string Id, string Name, string Kind, string Unit, double? Value);
