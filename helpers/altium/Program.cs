using System.Drawing;
using System.Text;
using System.Text.Json;
using OriginalCircuit.AltiumSharp;
using OriginalCircuit.AltiumSharp.BasicTypes;
using OriginalCircuit.AltiumSharp.Records;

Encoding.RegisterProvider(CodePagesEncodingProvider.Instance);
Console.InputEncoding = Encoding.UTF8;
Console.OutputEncoding = new UTF8Encoding(false);
var jsonOptions = new JsonSerializerOptions { PropertyNameCaseInsensitive = true, PropertyNamingPolicy = JsonNamingPolicy.CamelCase };
try
{
    var selfTest = args.Length == 2 && args[0] == "--self-test";
    if (args.Length != 0 && !selfTest) throw new ArgumentException("Use stdin JSON, or --self-test <new-output-directory>.");
    var json = selfTest ? "" : await Console.In.ReadToEndAsync();
    if (json.Length > 50_000_000) throw new ArgumentException("Request exceeds 50 MB.");
    var request = selfTest ? Smoke.Request(args[1]) : JsonSerializer.Deserialize<Request>(json, jsonOptions) ?? throw new ArgumentException("Empty JSON request.");
    Result result;
    var standardOut = Console.Out; var standardError = Console.Error;
    using (var diagnostics = new StringWriter())
    {
        try { Console.SetOut(diagnostics); Console.SetError(diagnostics); result = Adapter.Export(request); }
        finally { Console.SetOut(standardOut); Console.SetError(standardError); }
        var messages = diagnostics.ToString().Split(['\r', '\n'], StringSplitOptions.RemoveEmptyEntries).Distinct().Select(s => "AltiumSharp diagnostic: " + s);
        result = result with { Warnings = result.Warnings.Concat(messages).ToArray() };
    }
    if (selfTest) Smoke.Verify(result);
    Console.WriteLine(JsonSerializer.Serialize(result, jsonOptions));
    return 0;
}
catch (Exception error)
{
    if (args.Contains("--self-test")) Console.Error.WriteLine(error.ToString());
    Console.Error.WriteLine(JsonSerializer.Serialize(new { error = error.Message }, jsonOptions));
    return 1;
}

static class Adapter
{
    static readonly Color Ink = Color.FromArgb(0, 0, 128);
    static Coord Mm(double n) { if (!double.IsFinite(n) || Math.Abs(n) > 10000) throw new ArgumentException("Geometry must be finite and within +/-10000 mm."); return Coord.FromMMs(n); }
    static Coord Positive(double n) { if (n <= 0) throw new ArgumentException("Geometry size must be positive."); return Mm(n); }
    static CoordPoint Pt(double x, double y) => new(Mm(x), Mm(-y));
    static int Unit(int n, int count) { n = n == 0 ? 1 : n; if (n < 1 || n > count) throw new ArgumentException("Invalid symbol unit."); return n; }
    static string Name(string n) { if (string.IsNullOrWhiteSpace(n) || n.Length > 100 || n.Any(c => c < 32 || "\\/:*?\"<>|".Contains(c)) || n is "." or "..") throw new ArgumentException($"Invalid library/component name '{n}'."); return n.Trim(); }
    static double Angle(double n) { if (!double.IsFinite(n)) throw new ArgumentException("Invalid rotation."); return ((-n % 360) + 360) % 360; }
    static TextOrientations PinAngle(double n)
    {
        var a = Angle(n); var cardinal = Math.Round(a / 90) * 90;
        if (Math.Abs(a - cardinal) > .001) throw new ArgumentException("Schematic pins need cardinal rotations.");
        return ((int)cardinal % 360) switch { 0 => TextOrientations.None, 90 => TextOrientations.Rotated, 180 => TextOrientations.Flipped, _ => TextOrientations.Rotated | TextOrientations.Flipped };
    }
    static Layer LayerFor(string n, HashSet<string> warnings)
    {
        byte id = n switch { "F.Cu" => 1, "B.Cu" => 32, "*.Cu" => 74, "F.SilkS" => 21, "B.SilkS" => 22, "F.Paste" => 23, "B.Paste" => 24, "F.Mask" => 29, "B.Mask" => 30, "F.CrtYd" => 57, "B.CrtYd" => 58, "F.Fab" => 59, "B.Fab" => 60, "Edge.Cuts" => 56, "Dwgs.User" => 61, _ => 0 };
        if (id == 0) throw new ArgumentException($"Unsupported PCB layer '{n}'; export stopped to preserve layer meaning.");
        if (n is "F.CrtYd" or "B.CrtYd" or "F.Fab" or "B.Fab" or "Dwgs.User") warnings.Add("Courtyard/fabrication graphics use Mechanical 1/2/3/4/5; configure their layer roles in Altium.");
        return Layer.FromByte(id);
    }
    static PinElectricalType Electrical(string? source, HashSet<string> warnings)
    {
        var n = source?.Trim().ToLowerInvariant() ?? "unspecified";
        if (n is "free" or "unspecified" or "no_connect" or "")
        {
            warnings.Add($"Source pin type '{(n == "" ? "unspecified" : n)}' maps to Altium Passive; review ERC semantics. The original type is retained in the pin description.");
            return PinElectricalType.Passive;
        }
        if (n is "power_in" or "power_out") warnings.Add("Source power_in and power_out both map to Altium Power; review power-pin ERC semantics.");
        return n switch
        {
            "input" => PinElectricalType.Input, "output" => PinElectricalType.Output, "bidirectional" or "bidi" or "inputoutput" => PinElectricalType.InputOutput,
            "open_collector" or "opencollector" => PinElectricalType.OpenCollector, "open_emitter" or "openemitter" => PinElectricalType.OpenEmitter,
            "tri_state" or "tristate" or "highz" => PinElectricalType.HiZ, "power_in" or "power_out" or "power" => PinElectricalType.Power, "passive" => PinElectricalType.Passive,
            _ => throw new ArgumentException($"Unsupported electrical type '{n}'.")
        };
    }

    public static Result Export(Request request)
    {
        if (request.Parts is not { Count: > 0 and <= 500 }) throw new ArgumentException("parts must contain 1 to 500 components.");
        if (!request.IncludeSymbol && !request.IncludeFootprint) throw new ArgumentException("Select symbols and/or footprints.");
        if (string.IsNullOrWhiteSpace(request.OutputDirectory)) throw new ArgumentException("outputDirectory is required.");
        var dir = Path.GetFullPath(request.OutputDirectory); var name = Name(request.LibraryName);
        var warnings = new HashSet<string>(); var pcb = new PcbLib(); var sch = new SchLib();
        var pcbNames = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase); var schNames = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        foreach (var part in request.Parts)
        {
            Normalize(part);
            var fpName = Name(string.IsNullOrWhiteSpace(part.Footprint.Name) ? part.Name : part.Footprint.Name);
            if (request.IncludeFootprint)
            {
                if (part.Footprint.Pads.Count == 0) throw new ArgumentException($"{part.Name}: footprint has no pads.");
                var fingerprint = JsonSerializer.Serialize(part.Footprint);
                if (pcbNames.TryGetValue(fpName, out var previous))
                {
                    if (fingerprint != previous) throw new ArgumentException($"Footprint name '{fpName}' is used by different geometries.");
                }
                else { pcbNames.Add(fpName, fingerprint); pcb.Add(Footprint(part, fpName, warnings)); }
            }
            if (request.IncludeSymbol)
            {
                var sn = Name(string.IsNullOrWhiteSpace(part.Symbol.Name) ? part.Name : part.Symbol.Name);
                if (!schNames.Add(sn)) throw new ArgumentException($"Duplicate symbol name '{sn}'.");
                sch.Add(Symbol(part, sn, fpName, request.IncludeFootprint, warnings));
            }
            if (part.Model is not null) warnings.Add("3D model embedding is not implemented; this Altium export contains 2D geometry only.");
        }
        Directory.CreateDirectory(dir);
        var pending = new List<(string Temp, string Target)>();
        try
        {
            if (request.IncludeFootprint)
            {
                var target = Path.Combine(dir, name + ".PcbLib");
                if (File.Exists(target)) throw new IOException($"Output already exists: {target}");
                var temp = target + "." + Guid.NewGuid().ToString("N") + ".tmp"; pending.Add((temp, target));
                using (var w = new PcbLibWriter()) w.Write(pcb, temp, false);
                using var reader = new PcbLibReader(); var back = reader.Read(temp);
                if (reader.Errors.Count != 0 || back.Items.Count != pcb.Items.Count) throw new InvalidDataException("PcbLib write/read-back validation failed.");
            }
            if (request.IncludeSymbol)
            {
                var target = Path.Combine(dir, name + ".SchLib");
                if (File.Exists(target)) throw new IOException($"Output already exists: {target}");
                var temp = target + "." + Guid.NewGuid().ToString("N") + ".tmp"; pending.Add((temp, target));
                using (var w = new SchLibWriter()) w.Write(sch, temp, false);
                using var reader = new SchLibReader(); var back = reader.Read(temp);
                if (reader.Errors.Count != 0 || back.Items.Count != sch.Items.Count) throw new InvalidDataException("SchLib write/read-back validation failed.");
            }
            foreach (var (temp, target) in pending) File.Move(temp, target, false);
        }
        finally { foreach (var (temp, _) in pending) if (File.Exists(temp)) File.Delete(temp); }
        warnings.Add("Native libraries passed writer/reader checks; placement, ERC/DRC and 3D require verification in the intended Altium version.");
        return new Result(dir, pending.Select(p => p.Target).ToArray(), warnings.ToArray());
    }

    static void Normalize(Part part)
    {
        if (part is null) throw new ArgumentException("A part cannot be null.");
        part.Name ??= ""; part.Description ??= ""; part.Footprint ??= new(); part.Symbol ??= new();
        var f = part.Footprint; f.Pads ??= []; f.Lines ??= []; f.Circles ??= []; f.Polygons ??= []; f.Texts ??= [];
        var s = part.Symbol; s.Pins ??= []; s.Lines ??= []; s.Rects ??= []; s.Circles ??= []; s.Polygons ??= [];
        foreach (var p in f.Pads) { p.Points ??= []; if (p.Drill < 0 || p.DrillWidth < 0) throw new ArgumentException("Drill sizes cannot be negative."); }
        foreach (var p in f.Polygons) p.Points ??= [];
        foreach (var p in s.Polygons) p.Points ??= [];
    }

    static PcbComponent Footprint(Part part, string name, HashSet<string> warnings)
    {
        var src = part.Footprint; var dst = new PcbComponent { Pattern = name, Description = part.Description };
        foreach (var p in src.Pads)
        {
            var th = p.Drill > 0 || p.Type is "thru_hole" or "np_thru_hole";
            if (p.Shape == "custom" || p.Points.Count > 0) throw new ArgumentException($"{name} pad {p.Number}: custom polygon pads are not supported yet.");
            var shape = p.Shape switch { "rect" => PcbPadShape.Rectangular, "roundrect" => PcbPadShape.RoundedRectangle, "circle" or "oval" => PcbPadShape.Round, _ => throw new ArgumentException($"Unsupported pad shape '{p.Shape}'.") };
            var size = new CoordPoint(Positive(p.Width), Positive(p.Height));
            var pad = new PcbPad(th ? PcbPadTemplate.Tht : p.Layer == "B.Cu" ? PcbPadTemplate.SmtBottom : PcbPadTemplate.SmtTop)
            {
                Designator = p.Number, Location = Pt(p.X, p.Y), SizeTop = size, SizeMiddle = size, SizeBottom = size,
                ShapeTop = shape, ShapeMiddle = shape, ShapeBottom = shape, Rotation = Angle(p.Rotation),
                HoleSize = Mm(Math.Max(0, p.Drill)), IsPlated = p.Type != "np_thru_hole", Layer = LayerFor(th ? "*.Cu" : p.Layer, warnings)
            };
            if (p.Shape == "roundrect") { pad.CornerRadiusTop = pad.CornerRadiusMid = pad.CornerRadiusBot = 25; warnings.Add("Rounded rectangular pad corner radius uses 25%; the input model has no explicit radius."); }
            if (p.DrillWidth > p.Drill && th) { pad.HoleShape = PcbPadHoleShape.Slot; pad.HoleSlotLength = Mm(p.DrillWidth); }
            dst.Add(pad);
        }
        foreach (var l in src.Lines) dst.Add(new PcbTrack { Start = Pt(l.X1, l.Y1), End = Pt(l.X2, l.Y2), Width = Positive(l.Width), Layer = LayerFor(l.Layer, warnings) });
        foreach (var c in src.Circles) dst.Add(new PcbArc { Location = Pt(c.X, c.Y), Radius = Positive(c.Radius), StartAngle = 0, EndAngle = 360, Width = Positive(c.Width), Layer = LayerFor(c.Layer, warnings) });
        foreach (var p in src.Polygons)
        {
            if (p.Points.Count < 3) throw new ArgumentException("PCB polygon needs at least three vertices.");
            if (p.Fill) { var r = new PcbRegion { Layer = LayerFor(p.Layer, warnings) }; r.Outline.AddRange(p.Points.Select(x => Pt(x.X, x.Y))); dst.Add(r); }
            else for (var i = 0; i < p.Points.Count; i++) { var a = p.Points[i]; var b = p.Points[(i + 1) % p.Points.Count]; dst.Add(new PcbTrack { Start = Pt(a.X, a.Y), End = Pt(b.X, b.Y), Width = Positive(p.Width), Layer = LayerFor(p.Layer, warnings) }); }
        }
        foreach (var t in src.Texts) dst.Add(new PcbText { Text = t.Text, Corner1 = Pt(t.X, t.Y), Height = Positive(t.Size), StrokeWidth = Mm(.1), Rotation = Angle(t.Rotation), Layer = LayerFor(t.Layer, warnings) });
        return dst;
    }

    static SchComponent Symbol(Part part, string name, string fp, bool link, HashSet<string> warnings)
    {
        var src = part.Symbol;
        if (src.Pins.Count == 0) throw new ArgumentException($"{name}: symbol has no pins.");
        var units = Math.Max(1, src.Units); if (units > 100) throw new ArgumentException("Maximum 100 symbol units.");
        var dst = new SchComponent { LibReference = name, ComponentDescription = part.Description };
        while (dst.PartCount < units) dst.AddPart();
        dst.CurrentPartId = 1;
        dst.Add(new SchDesignator());
        dst.Add(new SchParameter { Name = "Comment" });
        dst.Add(new SchImplementationList());
        dst.Designator.Text = (string.IsNullOrWhiteSpace(src.Reference) ? "U" : src.Reference.TrimEnd('?')) + "?";
        dst.Designator.Location = Pt(0, -5); dst.Comment.Text = part.Name; dst.Comment.IsHidden = true;
        foreach (var p in src.Pins) dst.Add(new SchPin { Name = p.Name, Designator = p.Number, Description = "Source electrical type: " + (p.ElectricalType ?? "unspecified"), Location = Pt(p.X, p.Y), PinLength = Positive(p.Length), Orientation = PinAngle(p.Rotation), Electrical = Electrical(p.ElectricalType, warnings), OwnerPartId = Unit(p.Unit, units), Color = Ink, IsNameVisible = true, IsDesignatorVisible = true });
        foreach (var r in src.Rects) dst.Add(new SchRectangle { Location = Pt(r.X, r.Y), Corner = Pt(r.X + r.Width, r.Y + r.Height), Color = Ink, AreaColor = Color.White, LineWidth = LineWidth.Small, IsSolid = r.Fill, OwnerPartId = Unit(r.Unit, units) });
        foreach (var l in src.Lines) dst.Add(new SchLine { Location = Pt(l.X1, l.Y1), Corner = Pt(l.X2, l.Y2), Color = Ink, LineWidth = LineWidth.Small, OwnerPartId = Unit(l.Unit, units) });
        foreach (var c in src.Circles)
        {
            Positive(c.Radius);
            var vertices = Enumerable.Range(0, 65).Select(i => Pt(c.X + c.Radius * Math.Cos(i * Math.PI / 32), c.Y + c.Radius * Math.Sin(i * Math.PI / 32))).ToList();
            dst.Add(new SchPolyline { Vertices = vertices, Color = Ink, LineWidth = LineWidth.Small, OwnerPartId = Unit(c.Unit, units) });
            warnings.Add("Schematic circles are exported as 64-segment closed polylines.");
        }
        foreach (var p in src.Polygons)
        {
            var vertices = p.Points.Select(x => Pt(x.X, x.Y)).ToList(); if (vertices.Count < 2) throw new ArgumentException("Symbol polygon needs at least two vertices.");
            if (p.Fill) dst.Add(new SchPolygon { Vertices = vertices, IsSolid = true, Color = Ink, AreaColor = Color.White, LineWidth = LineWidth.Small, OwnerPartId = Unit(p.Unit, units) });
            else dst.Add(new SchPolyline { Vertices = vertices, Color = Ink, LineWidth = LineWidth.Small, OwnerPartId = Unit(p.Unit, units) });
        }
        foreach (var (key, value) in new[] { ("Manufacturer", part.Manufacturer), ("LCSC", part.LcscId), ("Source", part.Source), ("SourceURL", part.SourceUrl) }) if (!string.IsNullOrWhiteSpace(value)) dst.Add(new SchParameter { Name = key, Text = value, IsHidden = true });
        if (link)
        {
            // The stable package initializes its internal DataFile collection only on import.
            var parameters = new ParameterCollection();
            parameters.Add("RECORD", 45); parameters.Add("DATAFILECOUNT", 0); parameters.Add("ISCURRENT", true);
            var implementation = new SchImplementation(); implementation.ImportFromParameters(parameters);
            implementation.ModelName = fp; implementation.ModelType = "PCBLIB";
            implementation.IsCurrent = true; implementation.Description = "Package Studio footprint mapping"; implementation.OwnerPartId = -1;
            dst.Implementations.Add(implementation);
        }
        else warnings.Add("Symbols were exported without a footprint model association.");
        if (src.Lines.Count + src.Circles.Count + src.Polygons.Count > 0) warnings.Add("Schematic graphical line widths use the native Small width preset.");
        return dst;
    }
}

static class Smoke
{
    public static Request Request(string dir) => new()
    {
        OutputDirectory = dir, LibraryName = "PackageStudio_Smoke", IncludeSymbol = true, IncludeFootprint = true,
        Parts = [new Part { Name = "R_0603_10k", Description = "Independent resistor test geometry", Manufacturer = "Package Studio",
            Footprint = new FootprintData { Name = "R_0603", Pads = [new PadData { Number = "1", X = -.825, Width = .8, Height = .95 }, new PadData { Number = "2", X = .825, Width = .8, Height = .95 }], Lines = [new LineData { X1 = -.4, Y1 = -.5, X2 = .4, Y2 = -.5, Width = .12 }] },
            Symbol = new SymbolData { Name = "R_0603_10k", Reference = "R", Rects = [new RectData { X = -2.54, Y = -1.016, Width = 5.08, Height = 2.032 }], Pins = [new PinData { Number = "1", Name = "1", X = -5.08, Length = 2.54 }, new PinData { Number = "2", Name = "2", X = 5.08, Length = 2.54, Rotation = 180 }] } }]
    };
    public static void Verify(Result result)
    {
        using var pr = new PcbLibReader(); var pcb = pr.Read(result.Files.Single(f => f.EndsWith(".PcbLib")));
        var pads = pcb.Items.Single().GetPrimitivesOfType<PcbPad>(false).ToArray();
        if (pads.Length != 2 || pads[0].Designator != "1" || Math.Abs(pads[0].Location.X.ToMMs() + .825) > .00001 || Math.Abs(pads[0].SizeTop.Y.ToMMs() - .95) > .00001) throw new InvalidDataException("Pad read-back smoke check failed.");
        using var sr = new SchLibReader(); var sch = sr.Read(result.Files.Single(f => f.EndsWith(".SchLib")));
        var pins = sch.Items.Single().GetPrimitivesOfType<SchPin>(false).OrderBy(x => x.Designator).ToArray();
        if (pins.Length != 2 || Math.Abs(pins[0].GetCorner().X.ToMMs() + 2.54) > .00001 || Math.Abs(pins[1].GetCorner().X.ToMMs() - 2.54) > .00001 || sch.Items.Single().PartCount != 1) throw new InvalidDataException("Pin direction/unit read-back smoke check failed.");
        Console.Error.WriteLine("Smoke checks passed: CFB libraries, 2 pads with positions/sizes, 2 pin directions, one symbol unit.");
    }
}

sealed record Result(string Directory, string[] Files, string[] Warnings);
sealed class Request { public List<Part> Parts { get; set; } = []; public string OutputDirectory { get; set; } = ""; public string LibraryName { get; set; } = ""; public bool IncludeSymbol { get; set; } = true; public bool IncludeFootprint { get; set; } = true; }
sealed class Part { public string Name { get; set; } = ""; public string Description { get; set; } = ""; public string Manufacturer { get; set; } = ""; public string LcscId { get; set; } = ""; public string Source { get; set; } = ""; public string SourceUrl { get; set; } = ""; public FootprintData Footprint { get; set; } = new(); public SymbolData Symbol { get; set; } = new(); public JsonElement? Model { get; set; } }
sealed class FootprintData { public string Name { get; set; } = ""; public List<PadData> Pads { get; set; } = []; public List<LineData> Lines { get; set; } = []; public List<CircleData> Circles { get; set; } = []; public List<PolygonData> Polygons { get; set; } = []; public List<TextData> Texts { get; set; } = []; }
sealed class SymbolData { public string Name { get; set; } = ""; public string Reference { get; set; } = "U"; public int Units { get; set; } = 1; public List<PinData> Pins { get; set; } = []; public List<LineData> Lines { get; set; } = []; public List<RectData> Rects { get; set; } = []; public List<CircleData> Circles { get; set; } = []; public List<PolygonData> Polygons { get; set; } = []; }
sealed class PointData { public double X { get; set; } public double Y { get; set; } }
sealed class PadData { public string Number { get; set; } = ""; public double X { get; set; } public double Y { get; set; } public double Width { get; set; } public double Height { get; set; } public string Shape { get; set; } = "rect"; public double Rotation { get; set; } public double Drill { get; set; } public double DrillWidth { get; set; } public string Type { get; set; } = "smd"; public string Layer { get; set; } = "F.Cu"; public List<PointData> Points { get; set; } = []; }
sealed class PinData { public string Number { get; set; } = ""; public string Name { get; set; } = ""; public double X { get; set; } public double Y { get; set; } public double Length { get; set; } public double Rotation { get; set; } public string ElectricalType { get; set; } = "passive"; public int Unit { get; set; } }
sealed class LineData { public double X1 { get; set; } public double Y1 { get; set; } public double X2 { get; set; } public double Y2 { get; set; } public double Width { get; set; } public string Layer { get; set; } = "F.SilkS"; public int Unit { get; set; } }
sealed class RectData { public double X { get; set; } public double Y { get; set; } public double Width { get; set; } public double Height { get; set; } public bool Fill { get; set; } public int Unit { get; set; } }
sealed class CircleData { public double X { get; set; } public double Y { get; set; } public double Radius { get; set; } public double Width { get; set; } public string Layer { get; set; } = "F.SilkS"; public int Unit { get; set; } }
sealed class PolygonData { public List<PointData> Points { get; set; } = []; public double Width { get; set; } public string Layer { get; set; } = "F.SilkS"; public bool Fill { get; set; } public int Unit { get; set; } }
sealed class TextData { public string Text { get; set; } = ""; public double X { get; set; } public double Y { get; set; } public double Size { get; set; } public double Rotation { get; set; } public string Layer { get; set; } = "F.SilkS"; }
