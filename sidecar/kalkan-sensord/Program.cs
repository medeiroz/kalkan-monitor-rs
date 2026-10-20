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
//
// Light/full mode (commands read from stdin, one per line; unknown lines are
// ignored and EOF is harmless — the sidecar keeps running):
//     mode full           poll every hardware group (the default at start)
//     mode light          poll only cpu, board and controller plus the "need" list
//     need gpu,storage    groups light mode must also poll, among
//                         cpu,board,controller,gpu,storage,memory (replaces the list)
// The UI switches to light while its window is hidden, so disks (SMART), the GPU
// and the DIMMs are not woken every second. Groups are toggled on the polling
// thread, between two updates. A light line lists only the sensors of the
// polled groups and ends with `,"mode":"light"`; full lines are unchanged.

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

// A sensor picked for the CPU temperature or the fan: its value and the
// JSON-escaped string literal of its Identifier.
readonly record struct Pick(float Value, string IdJson);

// Per-sensor strings derived once instead of every second. `Name` is the
// source the derived strings were built from; if LHM ever renames the sensor
// the entry is rebuilt.
sealed class SensorInfo
{
    public required string Name;
    public required string Lower;   // Name.ToLowerInvariant()
    public required string IdJson;  // Str(Identifier.ToString())
    public required string NameJson; // Str(Name)
}

// Per-hardware JSON fragment `,"hardware":"…","hardware_type":"…"`.
sealed class HardwareInfo
{
    public required string Name;
    public required string Fragment;
}

// Mode requested over stdin; written by the stdin thread, applied by the loop.
sealed class ModeRequest
{
    public readonly object Lock = new();
    public bool Light;
    public HashSet<string> Need = new();
    public int Version;
    public readonly AutoResetEvent Wake = new(false);
}

static class Program
{
    static readonly string[] AllGroups = { "cpu", "board", "controller", "gpu", "storage", "memory" };
    static readonly string[] BaseGroups = { "cpu", "board", "controller" };

    static readonly string[] CpuTempKeys = { "tctl", "tdie", "package", "core (max", "core" };
    static readonly string[] FanKeys = { "pump", "cpu", "aio", "water", "liquid" };

    // Keyed by object identity: LHM keeps the same ISensor/IHardware instances
    // for the life of the Computer.
    static readonly Dictionary<ISensor, SensorInfo> SensorCache = new(ReferenceEqualityComparer.Instance);
    static readonly Dictionary<IHardware, HardwareInfo> HardwareCache = new(ReferenceEqualityComparer.Instance);

    [System.Runtime.InteropServices.DllImport("kernel32.dll")]
    static extern IntPtr GetCurrentProcess();

    [System.Runtime.InteropServices.DllImport("kernel32.dll", EntryPoint = "K32EmptyWorkingSet")]
    static extern bool K32EmptyWorkingSet(IntPtr process);

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

        // Reused every iteration (Clear keeps the capacity) instead of building
        // new builders/strings each second.
        var list = new StringBuilder(4096);
        var line = new StringBuilder(4096);
        bool trimmed = false;
        int ticks = 0;

        // Hardware is scanned in the order it had at startup (full mode), even
        // after groups were removed and re-added (LHM appends a re-added group
        // at the end), so the automatic picks do not depend on mode switches.
        var rank = new Dictionary<HardwareType, int>();
        foreach (IHardware hw in computer.Hardware)
            rank.TryAdd(hw.HardwareType, rank.Count);
        IHardware[] ordered = Array.Empty<IHardware>();
        bool reorder = true;
        computer.HardwareAdded += _ => Volatile.Write(ref reorder, true);
        computer.HardwareRemoved += _ => Volatile.Write(ref reorder, true);

        var request = new ModeRequest();
        new Thread(() => ReadCommands(request)) { IsBackground = true, Name = "stdin" }.Start();
        int applied = 0;
        bool light = false;

        try
        {
            while (true)
            {
                // Apply a requested mode change here, never while LHM updates.
                HashSet<string>? need = null;
                bool wantLight = false;
                lock (request.Lock)
                {
                    if (request.Version != applied)
                    {
                        applied = request.Version;
                        wantLight = request.Light;
                        need = new HashSet<string>(request.Need);
                    }
                }
                if (need is not null)
                {
                    light = wantLight;
                    var on = new HashSet<string>(light ? BaseGroups : AllGroups);
                    if (light) on.UnionWith(need);
                    if (Apply(computer, on))
                    {
                        // Removed hardware must not stay alive in the caches.
                        SensorCache.Clear();
                        HardwareCache.Clear();
                        Volatile.Write(ref reorder, true);
                        trimmed = false; // compact once more after the change
                    }
                }
                if (Volatile.Read(ref reorder))
                {
                    Volatile.Write(ref reorder, false);
                    ordered = computer.Hardware
                        .OrderBy(h => rank.TryGetValue(h.HardwareType, out int r) ? r : int.MaxValue)
                        .ToArray();
                }

                computer.Accept(visitor);

                Pick? cpuPref = null, tempAny = null;
                Pick? fanPref = null, fanAny = null;

                list.Clear();
                foreach (IHardware hw in ordered)
                    Scan(hw, ref cpuPref, ref tempAny, ref fanPref, ref fanAny, list);

                // The picked sensors are reported by Identifier (the same ids as
                // in "sensors"), so the Rust side never has to guess which sensor
                // a value came from.
                Pick? temp = cpuPref ?? tempAny;
                Pick? fan = fanPref ?? fanAny;

                line.Clear();
                line.Append("{\"cpu_temp_c\":"); AppendNum(line, temp?.Value);
                line.Append(",\"cpu_temp_id\":").Append(temp is Pick t ? t.IdJson : "null");
                line.Append(",\"fan_rpm\":"); AppendNum(line, fan?.Value);
                line.Append(",\"fan_id\":").Append(fan is Pick f ? f.IdJson : "null");
                line.Append(",\"sensors\":[").Append(list).Append(']');
                if (light) line.Append(",\"mode\":\"light\"");
                line.Append('}');
                stdout.WriteLine(line);
                stdout.Flush();

                // Once, after the first full pass: compact away the transient
                // garbage from LHM's hardware detection so the steady-state heap
                // starts small. Repeated only after a mode change (hardware
                // groups added/removed) — the loop itself allocates little.
                if (!trimmed)
                {
                    trimmed = true;
                    GC.Collect(2, GCCollectionMode.Aggressive, blocking: true, compacting: true);
                }
                // LHM's own hardware polling allocates garbage every second, and on
                // CPUs with a big cache the GC's gen0 budget is large, so memory
                // climbed for minutes before the first collection. Collect and
                // return the freed pages to Windows every 10 s to keep it flat.
                else if (++ticks % 10 == 0)
                {
                    GC.Collect(2, GCCollectionMode.Forced, blocking: true, compacting: false);
                    K32EmptyWorkingSet(GetCurrentProcess());
                }

                // A command wakes the loop early so a mode change applies at once.
                request.Wake.WaitOne(1000);
            }
        }
        finally
        {
            computer.Close();
        }
    }

    // Reads mode commands from stdin until EOF, then just stops reading (the
    // sidecar keeps running in the current mode).
    static void ReadCommands(ModeRequest request)
    {
        try
        {
            string? cmd;
            while ((cmd = Console.In.ReadLine()) is not null)
            {
                string[] parts = cmd.Trim().ToLowerInvariant()
                    .Split(' ', 2, StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries);
                if (parts.Length == 0) continue;
                lock (request.Lock)
                {
                    if (parts[0] == "mode" && parts.Length == 2 && parts[1] is "full" or "light")
                        request.Light = parts[1] == "light";
                    else if (parts[0] == "need")
                        request.Need = new HashSet<string>(
                            (parts.Length == 2 ? parts[1] : "")
                                .Split(',', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries)
                                .Where(g => Array.IndexOf(AllGroups, g) >= 0));
                    else
                        continue; // unknown command: ignored
                    request.Version++;
                }
                request.Wake.Set();
            }
        }
        catch (Exception)
        {
            // No usable stdin: stay in the current mode.
        }
    }

    // Enables exactly the hardware groups in `on`; true if anything changed.
    // LHM adds/removes (and closes) the group's hardware in each setter.
    static bool Apply(Computer c, HashSet<string> on)
    {
        bool changed = false;
        void Set(bool current, string group, Action<bool> set)
        {
            bool want = on.Contains(group);
            if (current != want) { set(want); changed = true; }
        }
        Set(c.IsCpuEnabled, "cpu", v => c.IsCpuEnabled = v);
        Set(c.IsMotherboardEnabled, "board", v => c.IsMotherboardEnabled = v);
        Set(c.IsControllerEnabled, "controller", v => c.IsControllerEnabled = v);
        Set(c.IsGpuEnabled, "gpu", v => c.IsGpuEnabled = v);
        Set(c.IsStorageEnabled, "storage", v => c.IsStorageEnabled = v);
        Set(c.IsMemoryEnabled, "memory", v => c.IsMemoryEnabled = v);
        return changed;
    }

    // Scans hardware + sub-hardware, accumulating temp/fan candidates and
    // appending every valid temp/fan sensor to `list` (comma-separated JSON objects).
    static void Scan(IHardware hw, ref Pick? cpuPref, ref Pick? tempAny,
                     ref Pick? fanPref, ref Pick? fanAny, StringBuilder list)
    {
        bool isCpu = hw.HardwareType == HardwareType.Cpu;
        HardwareInfo? hwInfo = null; // built lazily: only hardware with listed sensors needs it
        foreach (ISensor s in hw.Sensors)
        {
            if (s.Value is not float v || float.IsNaN(v) || float.IsInfinity(v)) continue;

            if (s.SensorType == SensorType.Temperature && v > 0f && v <= 150f)
            {
                SensorInfo info = GetInfo(s);
                string name = info.Lower;
                var pick = new Pick(v, info.IdJson);
                tempAny ??= pick;
                if (cpuPref is null && (isCpu || Matches(name, CpuTempKeys)))
                {
                    // on a CPU, prefer the most "package/die"-like label
                    if (isCpu && Matches(name, CpuTempKeys)) cpuPref = pick;
                    else if (!isCpu && Matches(name, CpuTempKeys)) cpuPref = pick;
                }
                Append(list, hwInfo ??= GetInfo(hw), info, "temp", v);
            }
            else if (s.SensorType == SensorType.Fan && v > 0f && v < 100000f)
            {
                SensorInfo info = GetInfo(s);
                var pick = new Pick(v, info.IdJson);
                fanAny ??= pick;
                if (fanPref is null && Matches(info.Lower, FanKeys)) fanPref = pick;
                Append(list, hwInfo ??= GetInfo(hw), info, "fan", v);
            }
        }
        foreach (IHardware sub in hw.SubHardware)
            Scan(sub, ref cpuPref, ref tempAny, ref fanPref, ref fanAny, list);
    }

    static SensorInfo GetInfo(ISensor s)
    {
        string name = s.Name;
        if (SensorCache.TryGetValue(s, out SensorInfo? info) && info.Name == name)
            return info;
        info = new SensorInfo
        {
            Name = name,
            Lower = name.ToLowerInvariant(),
            IdJson = Str(s.Identifier.ToString()),
            NameJson = Str(name),
        };
        SensorCache[s] = info;
        return info;
    }

    static HardwareInfo GetInfo(IHardware hw)
    {
        string name = hw.Name;
        if (HardwareCache.TryGetValue(hw, out HardwareInfo? info) && info.Name == name)
            return info;
        info = new HardwareInfo
        {
            Name = name,
            Fragment = ",\"hardware\":" + Str(name) + ",\"hardware_type\":" + Str(hw.HardwareType.ToString()),
        };
        HardwareCache[hw] = info;
        return info;
    }

    static void Append(StringBuilder list, HardwareInfo hw, SensorInfo s, string kind, float v)
    {
        if (list.Length > 0) list.Append(',');
        list.Append("{\"id\":").Append(s.IdJson)
            .Append(hw.Fragment)
            .Append(",\"name\":").Append(s.NameJson)
            .Append(",\"kind\":\"").Append(kind)
            .Append("\",\"value\":");
        AppendNum(list, v);
        list.Append('}');
    }

    // JSON string literal, ASCII-only: whatever the console code page, the Rust
    // side always receives valid UTF-8. A lone UTF-16 surrogate (which some
    // drivers leave in corrupted names) is not valid JSON text for the parser,
    // so it is replaced by U+FFFD instead of being escaped as-is.
    // Only called when (re)building the per-sensor/per-hardware caches.
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

    // Same text as f.ToString("0.##", InvariantCulture), formatted straight into
    // the builder (no intermediate string); "null" when there is no value.
    static void AppendNum(StringBuilder sb, float? v)
    {
        if (v is float f) sb.Append(CultureInfo.InvariantCulture, $"{f:0.##}");
        else sb.Append("null");
    }
}
