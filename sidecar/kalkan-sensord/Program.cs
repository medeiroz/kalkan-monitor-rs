// kalkan-sensord — Windows sensor sidecar based on LibreHardwareMonitor.
//
// Every second, emits one JSON line on stdout with the CPU temperature and the
// fan/pump RPM, picked by label, in the same spirit as the HWiNFO parser on the
// Rust side:
//     {"cpu_temp_c":42.0,"cpu_temp_id":"/amdcpu/0/temperature/2",
//      "fan_rpm":2400,"fan_id":"/lpc/nct6799d/0/fan/1","sensors":[...]}
// "cpu_temp_id" / "fan_id" are the Identifiers of the sensors those two values
// were picked from (null when nothing was picked); they match "id" in "sensors".
// "sensors" lists every Temperature and Fan sensor on all hardware (CPU, GPU,
// motherboard/SuperIO, controllers, storage, memory), each as
//     {"id":"/lpc/nct6799d/0/fan/1","hardware":"NCT6799D","hardware_type":"SuperIO",
//      "name":"Fan #2","kind":"fan","value":2400}
// so the UI can let the user pick which one feeds each display field. Strings
// are ASCII-only (non-ASCII is \uXXXX-escaped). Unknown values are emitted as
// null. If the engine fails to open (no admin / driver blocked), the reason is written to stderr and the process exits with a
// non-zero code — the Rust process treats that as "no data" and falls back.
//
// Fixed 1 s cadence so nothing gets overloaded (matches the app's loop).

using System.Globalization;
using System.Text;
using LibreHardwareMonitor.Hardware;

// Updates hardware and sub-hardware (e.g. the motherboard Super-I/O lives in subhw).
sealed class UpdateVisitor : IVisitor
{
    public void VisitComputer(IComputer computer) => computer.Traverse(this);
    public void VisitHardware(IHardware hardware)
    {
        hardware.Update();
        foreach (IHardware sub in hardware.SubHardware) sub.Accept(this);
    }
    public void VisitSensor(ISensor sensor) { }
    public void VisitParameter(IParameter parameter) { }
}

// A sensor picked for the CPU temperature or the fan: its value and Identifier.
readonly record struct Pick(float Value, string Id);

static class Program
{
    static readonly string[] CpuTempKeys = { "tctl", "tdie", "package", "core (max", "core" };
    static readonly string[] FanKeys = { "pump", "cpu", "aio", "water", "liquid" };

    static int Main(string[] args)
    {
        bool dump = Array.IndexOf(args, "--dump") >= 0;
        var computer = new Computer
        {
            IsCpuEnabled = true,
            IsMotherboardEnabled = true, // Super-I/O: fans
            IsControllerEnabled = true,  // fan controllers (e.g. AIO)
            IsGpuEnabled = true,         // GPU temperatures / fans
            IsStorageEnabled = true,     // drive temperatures
            IsMemoryEnabled = true,      // DIMM temperatures
        };

        try
        {
            computer.Open();
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine("kalkan-sensord: failed to open LibreHardwareMonitor: " + ex.Message);
            return 2;
        }

        var visitor = new UpdateVisitor();
        var stdout = Console.Out;

        if (dump)
        {
            computer.Accept(visitor);
            foreach (IHardware hw in computer.Hardware) Dump(hw, 0);
            computer.Close();
            return 0;
        }

        try
        {
            while (true)
            {
                computer.Accept(visitor);

                Pick? cpuPref = null, tempAny = null;
                Pick? fanPref = null, fanAny = null;

                var list = new StringBuilder();
                foreach (IHardware hw in computer.Hardware)
                    Scan(hw, ref cpuPref, ref tempAny, ref fanPref, ref fanAny, list);

                // The picked sensors are reported by Identifier (the same ids as
                // in "sensors"), so the Rust side never has to guess which sensor
                // a value came from.
                Pick? temp = cpuPref ?? tempAny;
                Pick? fan = fanPref ?? fanAny;

                stdout.WriteLine(
                    "{\"cpu_temp_c\":" + Num(temp?.Value) + ",\"cpu_temp_id\":" + Id(temp) +
                    ",\"fan_rpm\":" + Num(fan?.Value) + ",\"fan_id\":" + Id(fan) +
                    ",\"sensors\":[" + list + "]}");
                stdout.Flush();

                Thread.Sleep(1000);
            }
        }
        finally
        {
            computer.Close();
        }
    }

    // Scans hardware + sub-hardware, accumulating temp/fan candidates and
    // appending every valid temp/fan sensor to `list` (comma-separated JSON objects).
    static void Scan(IHardware hw, ref Pick? cpuPref, ref Pick? tempAny,
                     ref Pick? fanPref, ref Pick? fanAny, StringBuilder list)
    {
        bool isCpu = hw.HardwareType == HardwareType.Cpu;
        foreach (ISensor s in hw.Sensors)
        {
            if (s.Value is not float v || float.IsNaN(v) || float.IsInfinity(v)) continue;
            string name = s.Name.ToLowerInvariant();

            if (s.SensorType == SensorType.Temperature && v > 0f && v <= 150f)
            {
                var pick = new Pick(v, s.Identifier.ToString());
                tempAny ??= pick;
                if (cpuPref is null && (isCpu || Matches(name, CpuTempKeys)))
                {
                    // on a CPU, prefer the most "package/die"-like label
                    if (isCpu && Matches(name, CpuTempKeys)) cpuPref = pick;
                    else if (!isCpu && Matches(name, CpuTempKeys)) cpuPref = pick;
                }
                Append(list, hw, s, "temp", v);
            }
            else if (s.SensorType == SensorType.Fan && v > 0f && v < 100000f)
            {
                var pick = new Pick(v, s.Identifier.ToString());
                fanAny ??= pick;
                if (fanPref is null && Matches(name, FanKeys)) fanPref = pick;
                Append(list, hw, s, "fan", v);
            }
        }
        foreach (IHardware sub in hw.SubHardware)
            Scan(sub, ref cpuPref, ref tempAny, ref fanPref, ref fanAny, list);
    }

    static void Append(StringBuilder list, IHardware hw, ISensor s, string kind, float v)
    {
        if (list.Length > 0) list.Append(',');
        list.Append("{\"id\":").Append(Str(s.Identifier.ToString()))
            .Append(",\"hardware\":").Append(Str(hw.Name))
            .Append(",\"hardware_type\":").Append(Str(hw.HardwareType.ToString()))
            .Append(",\"name\":").Append(Str(s.Name))
            .Append(",\"kind\":\"").Append(kind)
            .Append("\",\"value\":").Append(Num(v)).Append('}');
    }

    // JSON string literal, ASCII-only: whatever the console code page, the Rust
    // side always receives valid UTF-8. A lone UTF-16 surrogate (which some
    // drivers leave in corrupted names) is not valid JSON text for the parser,
    // so it is replaced by U+FFFD instead of being escaped as-is.
    static string Str(string text)
    {
        var sb = new StringBuilder("\"");
        for (int i = 0; i < text.Length; i++)
        {
            char c = text[i];
            if (char.IsHighSurrogate(c) && i + 1 < text.Length && char.IsLowSurrogate(text[i + 1]))
            {
                sb.Append("\\u").Append(((int)c).ToString("x4"))
                  .Append("\\u").Append(((int)text[++i]).ToString("x4"));
                continue;
            }
            switch (c)
            {
                case '"': sb.Append("\\\""); break;
                case '\\': sb.Append("\\\\"); break;
                case >= ' ' and <= '~': sb.Append(c); break;
                case var _ when char.IsSurrogate(c): sb.Append("\\ufffd"); break;
                default: sb.Append("\\u").Append(((int)c).ToString("x4")); break;
            }
        }
        return sb.Append('"').ToString();
    }

    // Diagnostics: prints all hardware/sensors that LHM sees.
    static void Dump(IHardware hw, int depth)
    {
        string pad = new string(' ', depth * 2);
        Console.WriteLine($"{pad}[{hw.HardwareType}] {hw.Name}");
        foreach (ISensor s in hw.Sensors)
        {
            string v = s.Value is float f ? f.ToString("0.##", CultureInfo.InvariantCulture) : "null";
            Console.WriteLine($"{pad}  - ({s.SensorType}) {s.Name} = {v}");
        }
        foreach (IHardware sub in hw.SubHardware) Dump(sub, depth + 1);
    }

    static bool Matches(string hay, string[] keys)
    {
        foreach (string k in keys) if (hay.Contains(k)) return true;
        return false;
    }

    // JSON for a picked sensor's id, or null when nothing was picked.
    static string Id(Pick? p) => p is Pick x ? Str(x.Id) : "null";

    static string Num(float? v) =>
        v is float f ? f.ToString("0.##", CultureInfo.InvariantCulture) : "null";
}
