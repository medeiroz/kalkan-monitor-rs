// kalkan-sensord — Windows sensor sidecar based on LibreHardwareMonitor.
//
// Every second, emits one JSON line on stdout with the CPU temperature and the
// fan/pump RPM, picked by label, in the same spirit as the HWiNFO parser on the
// Rust side:
//     {"cpu_temp_c":42.0,"fan_rpm":2400}
// Unknown values are emitted as null. If the engine fails to open (no admin /
// driver blocked), the reason is written to stderr and the process exits with a
// non-zero code — the Rust process treats that as "no data" and falls back.
//
// Fixed 1 s cadence so nothing gets overloaded (matches the app's loop).

using System.Globalization;
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

                float? cpuPref = null, tempAny = null;
                float? fanPref = null, fanAny = null;

                foreach (IHardware hw in computer.Hardware)
                    Scan(hw, ref cpuPref, ref tempAny, ref fanPref, ref fanAny);

                float? temp = cpuPref ?? tempAny;
                float? fan = fanPref ?? fanAny;

                stdout.WriteLine(
                    "{\"cpu_temp_c\":" + Num(temp) + ",\"fan_rpm\":" + Num(fan) + "}");
                stdout.Flush();

                Thread.Sleep(1000);
            }
        }
        finally
        {
            computer.Close();
        }
    }

    // Scans hardware + sub-hardware, accumulating temp/fan candidates.
    static void Scan(IHardware hw, ref float? cpuPref, ref float? tempAny,
                     ref float? fanPref, ref float? fanAny)
    {
        bool isCpu = hw.HardwareType == HardwareType.Cpu;
        foreach (ISensor s in hw.Sensors)
        {
            if (s.Value is not float v || float.IsNaN(v) || float.IsInfinity(v)) continue;
            string name = s.Name.ToLowerInvariant();

            if (s.SensorType == SensorType.Temperature && v > 0f && v <= 150f)
            {
                tempAny ??= v;
                if (cpuPref is null && (isCpu || Matches(name, CpuTempKeys)))
                {
                    // on a CPU, prefer the most "package/die"-like label
                    if (isCpu && Matches(name, CpuTempKeys)) cpuPref = v;
                    else if (!isCpu && Matches(name, CpuTempKeys)) cpuPref = v;
                }
            }
            else if (s.SensorType == SensorType.Fan && v > 0f && v < 100000f)
            {
                fanAny ??= v;
                if (fanPref is null && Matches(name, FanKeys)) fanPref = v;
            }
        }
        foreach (IHardware sub in hw.SubHardware)
            Scan(sub, ref cpuPref, ref tempAny, ref fanPref, ref fanAny);
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

    static string Num(float? v) =>
        v is float f ? f.ToString("0.##", CultureInfo.InvariantCulture) : "null";
}
