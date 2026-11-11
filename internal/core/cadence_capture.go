package core

import (
	"fmt"
	"math"
	"strings"
)

// Tcl words are double quoted and escape substitutions AND braces, since this
// code lives inside a braced catch body. User pin names never become Tcl code.
func tclString(s string) string {
	r := strings.NewReplacer("\\", "\\\\", "\"", "\\\"", "$", "\\$", "[", "\\[", "]", "\\]", "{", "\\{", "}", "\\}", "\r", "\\r", "\n", "\\n", "\x00", "")
	return "\"" + r.Replace(s) + "\""
}
func capturePoint(x, y float64) string { return fmt.Sprintf("[psPoint %s %s]", f(x), f(y)) }

// Dbo method signatures follow Cadence OrCAD Capture Tcl/Tk Extensions Rev 1.2,
// pp.215,287-291,334-336,599,726. Pin enum values are resolved from the running
// Capture's own exported DboValue constants; numeric enum values are not guessed.
func renderCadenceCapture(parts []Part, entries []string, libraryName string) (string, []string, error) {
	return renderCadenceCaptureMode(parts, entries, libraryName, "", false)
}

func renderCadenceCaptureMode(parts []Part, entries []string, libraryName, nonce string, appendLibrary bool) (string, []string, error) {
	warnings := []string{"Capture Tcl 已生成，尚未在目标 Cadence 版本执行；必须在 Capture 中回读所有单元并执行 ERC。", "Capture 默认采用100 DBU/inch兼容尺度（0.254mm/DBU），该默认尚未在本机文档/运行时验证；请在目标版核对网格和引脚长度，可用 -CaptureUnitsPerInch 覆盖。", "Capture 引脚类型从目标版本的 DboValue_* 常量解析；如该版本未公开相应常量，脚本会明确失败而不会使用猜测的枚举值。"}
	warnings = append(warnings, "Capture 图形线宽、字体和引脚号位置采用目标工具默认设置；请在目标版本中核对显示效果。")
	var b strings.Builder
	b.WriteString(capturePrelude)
	fmt.Fprintf(&b, "\nset psOutput [file join $psDirectory %s]\n", tclString(libraryName+".olb"))
	if appendLibrary {
		b.WriteString("if {![file exists $psOutput]} {error \"Staged Capture library is missing\"}\nset psCode [catch {\n set psSession [DboTclHelper_sCreateSession]\n set psStatus [DboState]\n set psLibrary [psObject [$psSession GetLib [DboTclHelper_sMakeCString $psOutput] $psStatus] $psStatus GetLib]\n")
	} else {
		b.WriteString("if {[file exists $psOutput]} {error \"Capture library already exists; use a fresh batch\"}\nset psCode [catch {\n set psSession [DboTclHelper_sCreateSession]\n set psStatus [DboState]\n set psLibrary [psObject [$psSession CreateLib [DboTclHelper_sMakeCString $psOutput] $psStatus] $psStatus CreateLib]\n")
	}
	if nonce != "" {
		b.WriteString(" set psBefore [psLibraryInventory $psLibrary $psStatus]\n set psBeforeFile [open [file join $psDirectory before-inventory.tcldata] w]\n fconfigure $psBeforeFile -encoding utf-8\n puts $psBeforeFile [list $psBefore]\n close $psBeforeFile\n")
		// Check all conflicts before making the first change to the staged copy.
		for _, entry := range entries {
			fmt.Fprintf(&b, " if {[$psLibrary PackageExists [DboTclHelper_sMakeCString %s] $psStatus]} {error %s}\n psState $psStatus PackageExists\n", tclString(entry), tclString("Package already exists; append refuses conflicts: "+entry))
		}
	}
	for pi, p := range parts {
		units := p.Symbol.Units
		if units < 1 {
			units = 1
		}
		if units > 100 {
			return "", warnings, fmt.Errorf("%s：Capture单元数超过100", p.Name)
		}
		entry := entries[pi]
		ref := strings.TrimRight(p.Symbol.Reference, "?")
		if ref == "" {
			ref = "U"
		}
		fmt.Fprintf(&b, " set psPackage [psObject [$psLibrary NewPackage [DboTclHelper_sMakeCString %s] $psStatus] $psStatus NewPackage]\n $psPackage SetReferenceTemplate [DboTclHelper_sMakeCString %s]\n $psPackage SetPCBFootprint [DboTclHelper_sMakeCString %s]\n", tclString(entry), tclString(ref+"?"), tclString(entry))
		if nonce != "" {
			fmt.Fprintf(&b, " psState [$psPackage SetEffectivePropStringValue [DboTclHelper_sMakeCString \"PackageStudioNonce\"] [DboTclHelper_sMakeCString %s]] SetNonce\n", tclString(nonce))
		}
		for u := 1; u <= units; u++ {
			unitName := fmt.Sprintf("%s_UNIT%d", entry, u)
			designator := ""
			if units > 1 {
				if u <= 26 {
					designator = string(rune('A' + u - 1))
				} else {
					designator = fmt.Sprintf("U%d", u)
				}
			}
			fmt.Fprintf(&b, " set psCell [psObject [$psLibrary NewCell [DboTclHelper_sMakeCString %s] $psStatus] $psStatus NewCell]\n set psSymbol [psObject [$psLibrary NewPart [DboTclHelper_sMakeCString %s] $psStatus] $psStatus NewPart]\n $psCell AddPart $psSymbol\n $psSymbol SetCellPtr $psCell\n $psSymbol SetPackagePtr $psPackage\n $psSymbol SetReference [DboTclHelper_sMakeCString %s]\n $psSymbol SetPartValue [DboTclHelper_sMakeCString %s]\n $psSymbol SetPinNamesAreVisible 1\n $psSymbol SetPinNumbersAreVisible 1\n set psDevice [psObject [$psPackage NewDevice [DboTclHelper_sMakeCString %s] %d $psCell $psStatus] $psStatus NewDevice]\n", tclString(unitName), tclString(unitName), tclString(ref+"?"), tclString(p.Name), tclString(designator), u-1)
			if units > 1 {
				fmt.Fprintf(&b, " $psSymbol SetEffectivePropStringValue [DboTclHelper_sMakeCString \"Part Section\"] [DboTclHelper_sMakeCString %s]\n", tclString(designator))
			}
			pinPosition := 0
			for _, pin := range p.Symbol.Pins {
				if unit(pin.Unit) != u {
					continue
				}
				if math.Abs(pin.Rotation/90-math.Round(pin.Rotation/90)) > .00001 {
					return "", warnings, fmt.Errorf("%s：Capture 引脚仅支持0/90/180/270度方向", p.Name)
				}
				if pin.Length < 0 {
					return "", warnings, fmt.Errorf("%s：负引脚长度", p.Name)
				}
				a := pin.Rotation * math.Pi / 180
				startX, startY := pin.X+pin.Length*math.Cos(a), pin.Y+pin.Length*math.Sin(a)
				aliases, w, err := capturePinAliases(pin.ElectricalType)
				if err != nil {
					return "", warnings, fmt.Errorf("%s：%w", p.Name, err)
				}
				warnings = append(warnings, w...)
				fmt.Fprintf(&b, " set psStart %s\n set psHot %s\n set psPinType [psEnum {%s}]\n set psPin [psObject [$psSymbol NewSymbolPinScalar $psStatus [DboTclHelper_sMakeCString %s] $psPinType $psStart $psHot %d %d] $psStatus NewSymbolPinScalar]\n", capturePoint(startX, startY), capturePoint(pin.X, pin.Y), strings.Join(aliases, " "), tclString(pin.Name), boolInt(!pin.Hidden), pinPosition)
				fmt.Fprintf(&b, " $psPin SetIsNumberVisible 1\n $psPin SetIsClock %d\n $psPin SetIsDot %d\n", boolInt(strings.Contains(pin.GraphicStyle, "clock")), boolInt(strings.Contains(pin.GraphicStyle, "inverted")))
				fmt.Fprintf(&b, " set psNumberPosition [DboTclHelper_sMakeInt %d]\n psState [$psDevice NewPinNumber [DboTclHelper_sMakeCString %s] $psNumberPosition] NewPinNumber\n", pinPosition, tclString(pin.Number))
				fmt.Fprintf(&b, " set psNumberDisplay [psObject [$psPin NewDisplayProp $psStatus [DboTclHelper_sMakeCString \"Number\"] %s 0 [DboTclHelper_sMakeLOGFONT] 48] $psStatus NewDisplayProp]\n $psNumberDisplay SetDisplayType 1\n", capturePoint((pin.X+startX)/2, (pin.Y+startY)/2-.254))
				if pin.GraphicStyle != "" && pin.GraphicStyle != "line" && pin.GraphicStyle != "clock" && pin.GraphicStyle != "inverted" && pin.GraphicStyle != "inverted_clock" {
					warnings = append(warnings, "Capture 部分特殊引脚图形只保留时钟/反相标记，请核对原始引脚样式。")
				}
				pinPosition++
			}
			if pinPosition == 0 {
				return "", warnings, fmt.Errorf("%s：第%d单元没有引脚", p.Name, u)
			}
			for _, r := range p.Symbol.Rects {
				if unit(r.Unit) != u {
					continue
				}
				if r.Width <= 0 || r.Height <= 0 {
					return "", warnings, fmt.Errorf("Capture符号矩形尺寸无效")
				}
				fmt.Fprintf(&b, " psObject [$psSymbol NewBox $psStatus %s %s] $psStatus NewBox\n", capturePoint(r.X, r.Y), capturePoint(r.X+r.Width, r.Y+r.Height))
				if r.Fill {
					warnings = append(warnings, "Capture 矩形/多边形填充尚未映射，当前保留轮廓。")
				}
			}
			for _, l := range p.Symbol.Lines {
				if unit(l.Unit) != u {
					continue
				}
				fmt.Fprintf(&b, " psObject [$psSymbol NewLine $psStatus %s %s] $psStatus NewLine\n", capturePoint(l.X1, l.Y1), capturePoint(l.X2, l.Y2))
			}
			for _, c := range p.Symbol.Circles {
				if unit(c.Unit) != u {
					continue
				}
				if c.Radius <= 0 {
					return "", warnings, fmt.Errorf("Capture符号圆形半径无效")
				}
				fmt.Fprintf(&b, " psObject [$psSymbol NewEllipse $psStatus [psRect %s %s %s %s]] $psStatus NewEllipse\n", f(c.X-c.Radius), f(c.Y-c.Radius), f(c.X+c.Radius), f(c.Y+c.Radius))
			}
			for _, poly := range p.Symbol.Polygons {
				if unit(poly.Unit) != u {
					continue
				}
				if len(poly.Points) < 2 {
					return "", warnings, fmt.Errorf("Capture多边形顶点不足")
				}
				for i := 0; i < len(poly.Points)-1; i++ {
					a, z := poly.Points[i], poly.Points[i+1]
					fmt.Fprintf(&b, " psObject [$psSymbol NewLine $psStatus %s %s] $psStatus NewLine\n", capturePoint(a.X, a.Y), capturePoint(z.X, z.Y))
				}
				if poly.Fill {
					a, z := poly.Points[len(poly.Points)-1], poly.Points[0]
					fmt.Fprintf(&b, " psObject [$psSymbol NewLine $psStatus %s %s] $psStatus NewLine\n", capturePoint(a.X, a.Y), capturePoint(z.X, z.Y))
					warnings = append(warnings, "Capture 矩形/多边形填充尚未映射，当前保留轮廓。")
				}
			}
			bounds := p.Symbol.Bounds
			fmt.Fprintf(&b, " $psSymbol SetBoundingBox [psRect %s %s %s %s]\n", f(bounds.X), f(bounds.Y), f(bounds.X+bounds.Width), f(bounds.Y+bounds.Height))
		}
		fmt.Fprintf(&b, " $psPackage SetEffectivePropStringValue [DboTclHelper_sMakeCString \"Description\"] [DboTclHelper_sMakeCString %s]\n psState [$psLibrary SavePackageAll $psPackage] SavePackageAll\n", tclString(p.Description))
	}
	b.WriteString(" psState [$psSession SaveLib $psLibrary] SaveLib\n $psSession RemoveLib $psLibrary\n DboTclHelper_sDeleteSession $psSession\n if {![file exists $psOutput] || [file size $psOutput] <= 0} {error \"Capture did not write a nonempty OLB\"}\n set psMarker [open [file join $psDirectory capture-complete.txt] w]\n puts $psMarker \"state=done\"\n puts $psMarker \"unitsPerInch=$::psCaptureUnitsPerInch\"\n if {[llength [info commands GetProductVersion]]} {puts $psMarker \"engineVersion=[GetProductVersion]\"}\n close $psMarker\n} psError]\nif {$psCode} {\n set psLog [open [file join $psDirectory capture-error.txt] w]\n puts $psLog $psError\n if {[info exists ::errorInfo]} {puts $psLog $::errorInfo}\n close $psLog\n return -code error $psError\n}\nputs \"Package Studio Capture library saved; validate all gates and pin mappings in Capture.\"\n")
	return b.String(), uniqueCadenceWarnings(warnings), nil
}
func boolInt(v bool) int {
	if v {
		return 1
	}
	return 0
}
func capturePinAliases(t string) ([]string, []string, error) {
	var names []string
	warnings := []string{}
	switch strings.ToLower(t) {
	case "input":
		names = []string{"INPUT", "IN"}
	case "output":
		names = []string{"OUTPUT", "OUT"}
	case "bidirectional":
		names = []string{"BIDIRECTIONAL", "BIDIR"}
	case "tri_state":
		names = []string{"TRISTATE", "TRI_STATE", "THREE_STATE"}
	case "passive":
		names = []string{"PASSIVE"}
	case "open_collector":
		names = []string{"OPENCOLLECTOR", "OPEN_COLLECTOR"}
	case "open_emitter":
		names = []string{"OPENEMITTER", "OPEN_EMITTER"}
	case "power_in", "power_out", "power":
		names = []string{"POWER"}
		warnings = append(warnings, "Capture power_in/power_out 均映射到目标库 Power 类型，请复核电源 ERC 语义。")
	case "unspecified", "free", "no_connect", "":
		names = []string{"PASSIVE"}
		warnings = append(warnings, "Capture unspecified/free/no_connect 保守映射 Passive，源类型仍保存在 parts.source.json，需复核 ERC 语义。")
	default:
		return nil, warnings, fmt.Errorf("不支持的 Capture 电气类型：%s", t)
	}
	return names, warnings, nil
}

const capturePrelude = `# Package Studio independent Capture Dbo generator.
# API: Cadence OrCAD Capture Tcl/Tk Extensions Rev 1.2.
# Run inside Capture; this script creates a private Dbo session and a new OLB.
# Default scale is an explicitly UNVERIFIED compatibility setting, not a claim
# that this development host has tested a Cadence release. Override as needed.
set psDirectory [file dirname [file normalize [info script]]]
if {[file exists [file join $psDirectory run-config.tcl]]} {source [file join $psDirectory run-config.tcl]}
if {![info exists ::psCaptureUnitsPerInch]} {set ::psCaptureUnitsPerInch 100.0}
if {![string is double -strict $::psCaptureUnitsPerInch] || $::psCaptureUnitsPerInch <= 0} {error "Invalid CaptureUnitsPerInch"}
foreach psCommand {DboState DboTclHelper_sCreateSession DboTclHelper_sDeleteSession DboTclHelper_sMakeCString DboTclHelper_sMakeCPoint DboTclHelper_sMakeCRect DboTclHelper_sMakeInt DboTclHelper_sMakeLOGFONT} {
 if {![llength [info commands $psCommand]]} {error "Capture Dbo Tcl API unavailable: $psCommand"}
}
proc psPoint {x y} {
 return [DboTclHelper_sMakeCPoint [expr {round($x * $::psCaptureUnitsPerInch / 25.4)}] [expr {round($y * $::psCaptureUnitsPerInch / 25.4)}]]
}
proc psRect {x1 y1 x2 y2} {
 set scale [expr {$::psCaptureUnitsPerInch / 25.4}]
 return [DboTclHelper_sMakeCRect [expr {round($x1*$scale)}] [expr {round($y1*$scale)}] [expr {round($x2*$scale)}] [expr {round($y2*$scale)}]]
}
proc psState {status context} {
 if {$status eq "" || $status eq "NULL"} {error "$context returned no status"}
 if {[$status Failed]} {error "$context failed"}
 return $status
}
proc psObject {object status context} {
 psState $status $context
 if {$object eq "" || $object eq "NULL" || $object eq "0"} {error "$context returned a null object"}
 return $object
}
proc psEnum {aliases} {
 foreach alias $aliases {
  foreach suffix [list $alias PIN_$alias ${alias}_PIN] {
   set variable ::DboValue_$suffix
   if {[info exists $variable]} {return [set $variable]}
  }
 }
 error "Capture exposes no matching DboValue pin type: $aliases. Inspect this release's exported constants; no numeric enum value was guessed."
}
`
