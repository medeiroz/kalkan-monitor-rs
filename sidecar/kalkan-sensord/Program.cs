// kalkan-sensord — sidecar de sensores (Windows) baseado no LibreHardwareMonitor.
//
// Emite, a cada 1 s, uma linha JSON no stdout com temperatura de CPU e RPM de
// fan/bomba escolhidos por rótulo, no mesmo espírito do parser do HWiNFO do lado
// Rust:
//     {"cpu_temp_c":42.0,"fan_rpm":2400}
// Valores desconhecidos saem como null. Se o motor não abrir (sem admin / driver
// bloqueado), escreve o motivo no stderr e sai com código != 0 — o processo Rust
// trata isso como "sem dados" e cai no fallback.
//
// Cadência fixa de 1 s para não sobrecarregar nada (casa com o laço do app).

using System.Globalization;
using LibreHardwareMonitor.Hardware;

// Atualiza hardware e subhardware (ex.: Super-I/O da placa-mãe fica em subhw).
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
            IsControllerEnabled = true,  // controladoras de fan (ex.: AIO)
        };

        try
        {
            computer.Open();
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine("kalkan-sensord: falha ao abrir o LibreHardwareMonitor: " + ex.Message);
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

    // Varre hardware + subhardware acumulando candidatos de temp/fan.
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
                    // numa CPU, prefira o rótulo mais "de pacote/die"
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

    // Diagnóstico: imprime todo o hardware/sensor que o LHM enxerga.
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
