// Package core contains the portable, execution-free geometry and library engine.
package core

type Point struct {
	X float64 `json:"x"`
	Y float64 `json:"y"`
}
type Bounds struct {
	X      float64 `json:"x"`
	Y      float64 `json:"y"`
	Width  float64 `json:"width"`
	Height float64 `json:"height"`
}
type Line struct {
	X1    float64 `json:"x1"`
	Y1    float64 `json:"y1"`
	X2    float64 `json:"x2"`
	Y2    float64 `json:"y2"`
	Width float64 `json:"width"`
	Layer string  `json:"layer"`
	Unit  int     `json:"unit,omitempty"`
}
type Circle struct {
	X      float64 `json:"x"`
	Y      float64 `json:"y"`
	Radius float64 `json:"radius"`
	Width  float64 `json:"width"`
	Layer  string  `json:"layer"`
	Unit   int     `json:"unit,omitempty"`
}
type Polygon struct {
	Points []Point `json:"points"`
	Width  float64 `json:"width"`
	Layer  string  `json:"layer"`
	Fill   bool    `json:"fill"`
	Unit   int     `json:"unit,omitempty"`
}
type Text struct {
	Text     string  `json:"text"`
	X        float64 `json:"x"`
	Y        float64 `json:"y"`
	Size     float64 `json:"size"`
	Rotation float64 `json:"rotation"`
	Layer    string  `json:"layer"`
}
type Rect struct {
	X      float64 `json:"x"`
	Y      float64 `json:"y"`
	Width  float64 `json:"width"`
	Height float64 `json:"height"`
	Fill   bool    `json:"fill"`
	Unit   int     `json:"unit,omitempty"`
}
type Pad struct {
	Number     string  `json:"number"`
	X          float64 `json:"x"`
	Y          float64 `json:"y"`
	Width      float64 `json:"width"`
	Height     float64 `json:"height"`
	Shape      string  `json:"shape"`
	Rotation   float64 `json:"rotation"`
	Drill      float64 `json:"drill,omitempty"`
	DrillWidth float64 `json:"drillWidth,omitempty"`
	Type       string  `json:"type"`
	Layer      string  `json:"layer"`
	Points     []Point `json:"points,omitempty"`
}

// Pin rotation is screen-coordinate direction from the electrical endpoint inward.
type Pin struct {
	Number         string  `json:"number"`
	Name           string  `json:"name"`
	X              float64 `json:"x"`
	Y              float64 `json:"y"`
	Length         float64 `json:"length"`
	Rotation       float64 `json:"rotation"`
	ElectricalType string  `json:"electricalType"`
	GraphicStyle   string  `json:"graphicStyle,omitempty"`
	Hidden         bool    `json:"hidden,omitempty"`
	Unit           int     `json:"unit,omitempty"`
}
type Footprint struct {
	Name     string    `json:"name"`
	Pads     []Pad     `json:"pads"`
	Lines    []Line    `json:"lines"`
	Circles  []Circle  `json:"circles"`
	Polygons []Polygon `json:"polygons"`
	Texts    []Text    `json:"texts"`
	Bounds   Bounds    `json:"bounds"`
}
type Symbol struct {
	Name      string    `json:"name"`
	Reference string    `json:"reference"`
	Pins      []Pin     `json:"pins"`
	Lines     []Line    `json:"lines"`
	Rects     []Rect    `json:"rects"`
	Circles   []Circle  `json:"circles"`
	Polygons  []Polygon `json:"polygons"`
	Units     int       `json:"units"`
	Bounds    Bounds    `json:"bounds"`
}
type Model struct {
	Prepared            bool      `json:"prepared,omitempty"`
	ExpectedBBoxMM      []float64 `json:"expectedBBoxMm,omitempty"`
	BBoxToleranceMM     float64   `json:"bboxToleranceMm,omitempty"`
	PlacementConvention string    `json:"placementConvention,omitempty"`
	SourceWidthMM       float64   `json:"sourceWidthMm,omitempty"`
	SourceHeightMM      float64   `json:"sourceHeightMm,omitempty"`
	Path                string    `json:"path"`
	UUID                string    `json:"uuid,omitempty"`
	X                   float64   `json:"x"`
	Y                   float64   `json:"y"`
	Z                   float64   `json:"z"`
	RotationX           float64   `json:"rotationX"`
	RotationY           float64   `json:"rotationY"`
	RotationZ           float64   `json:"rotationZ"`
	ScaleX              float64   `json:"scaleX"`
	ScaleY              float64   `json:"scaleY"`
	ScaleZ              float64   `json:"scaleZ"`
}
type Part struct {
	ID           string    `json:"id"`
	LCSCID       string    `json:"lcscId"`
	Name         string    `json:"name"`
	Description  string    `json:"description"`
	Manufacturer string    `json:"manufacturer"`
	Package      string    `json:"package"`
	Source       string    `json:"source"`
	SourceURL    string    `json:"sourceUrl,omitempty"`
	Attribution  string    `json:"attribution,omitempty"`
	Warnings     []string  `json:"warnings"`
	Footprint    Footprint `json:"footprint"`
	Symbol       Symbol    `json:"symbol"`
	Model        *Model    `json:"model,omitempty"`
}
type ExportResult struct {
	Directory         string             `json:"directory"`
	Files             []string           `json:"files"`
	Warnings          []string           `json:"warnings"`
	ArtifactType      string             `json:"artifactType,omitempty"`
	NativeGenerated   bool               `json:"nativeGenerated,omitempty"`
	RuntimeVerified   bool               `json:"runtimeVerified,omitempty"`
	BackupDirectory   string             `json:"backupDirectory,omitempty"`
	ValidationStatus  string             `json:"validationStatus,omitempty"`
	ModelAssociations []ModelAssociation `json:"modelAssociations,omitempty"`
}

// ModelAssociation describes a model-to-footprint relationship separately from
// native library generation. A prepared script is not an EDA-verified mapping.
type ModelAssociation struct {
	PartID           string    `json:"partId"`
	Entry            string    `json:"entry"`
	Target           string    `json:"target"`
	ModelFile        string    `json:"modelFile,omitempty"`
	ModelSHA256      string    `json:"modelSha256,omitempty"`
	Status           string    `json:"status"`
	Verified         bool      `json:"verified"`
	Message          string    `json:"message,omitempty"`
	ModelName        string    `json:"modelName,omitempty"`
	ExpectedBBoxMM   []float64 `json:"expectedBBoxMm,omitempty"`
	ActualBBoxMM     []float64 `json:"actualBBoxMm,omitempty"`
	GeometryVerified bool      `json:"geometryVerified"`
}

func emptyPart() Part {
	return Part{Warnings: []string{}, Footprint: Footprint{Pads: []Pad{}, Lines: []Line{}, Circles: []Circle{}, Polygons: []Polygon{}, Texts: []Text{}}, Symbol: Symbol{Pins: []Pin{}, Lines: []Line{}, Rects: []Rect{}, Circles: []Circle{}, Polygons: []Polygon{}, Units: 1}}
}

func DemoParts() []Part {
	p := emptyPart()
	p.ID = "demo-r0603"
	p.Name = "R_0603_10k"
	p.Description = "10 kΩ · 0603 · 两引脚电阻演示器件"
	p.Manufacturer = "演示数据"
	p.Package = "R_0603"
	p.Source = "内置独立绘制示例"
	p.Attribution = "Package Studio original demonstration geometry"
	p.Warnings = []string{"演示几何，仅用于体验和导出格式验证；用于生产前请核对实际器件手册。"}
	p.Footprint.Name = "R_0603"
	p.Footprint.Pads = []Pad{{Number: "1", X: -.825, Y: 0, Width: .8, Height: .95, Shape: "roundrect", Type: "smd", Layer: "F.Cu"}, {Number: "2", X: .825, Y: 0, Width: .8, Height: .95, Shape: "roundrect", Type: "smd", Layer: "F.Cu"}}
	p.Footprint.Lines = []Line{{X1: -.4, Y1: -.5, X2: .4, Y2: -.5, Width: .12, Layer: "F.SilkS"}, {X1: -.4, Y1: .5, X2: .4, Y2: .5, Width: .12, Layer: "F.SilkS"}, {X1: -1.5, Y1: -.75, X2: 1.5, Y2: -.75, Width: .05, Layer: "F.CrtYd"}, {X1: 1.5, Y1: -.75, X2: 1.5, Y2: .75, Width: .05, Layer: "F.CrtYd"}, {X1: 1.5, Y1: .75, X2: -1.5, Y2: .75, Width: .05, Layer: "F.CrtYd"}, {X1: -1.5, Y1: .75, X2: -1.5, Y2: -.75, Width: .05, Layer: "F.CrtYd"}}
	p.Symbol.Name = p.Name
	p.Symbol.Reference = "R"
	p.Symbol.Rects = []Rect{{X: -2.54, Y: -1.016, Width: 5.08, Height: 2.032, Unit: 1}}
	p.Symbol.Pins = []Pin{{Number: "1", Name: "1", X: -5.08, Y: 0, Length: 2.54, Rotation: 0, ElectricalType: "passive", Unit: 1}, {Number: "2", Name: "2", X: 5.08, Y: 0, Length: 2.54, Rotation: 180, ElectricalType: "passive", Unit: 1}}
	updateBounds(&p)
	return []Part{p}
}
