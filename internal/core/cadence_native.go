package core

import (
	"encoding/json"
	"errors"
	"fmt"
	"math"
	"os"
	"path/filepath"
	"regexp"
	"strings"
)

// CadenceNativePlan describes engine jobs inside a private staging batch. The
// existence of these scripts is never evidence that a native library is valid.
type CadenceNativePlan struct {
	Nonce                                         string
	Entries                                       []string
	CaptureLibrary, CaptureBuild, CaptureReadback string
	CaptureBuildReport, CaptureReadbackReport     string
	Allegro                                       []CadenceNativeAllegro
}
type CadenceNativeAllegro struct {
	CadenceAllegroJob
	BuildReport, ReadbackScript, ReadbackReport, CompiledScript, CompiledReport string
}

func CadenceEntryNames(parts []Part) []string {
	entries := make([]string, len(parts))
	used := map[string]bool{}
	for i, p := range parts {
		entry := cadenceName(p.Name)
		if used[strings.ToLower(entry)] {
			entry = cadenceName(p.Name + "_" + p.ID)
		}
		base := entry
		for n := 2; used[strings.ToLower(entry)]; n++ {
			entry = fmt.Sprintf("%s_%d", base, n)
		}
		used[strings.ToLower(entry)] = true
		entries[i] = entry
	}
	return entries
}

// PrepareCadenceNative adds separate reopen/readback jobs to an independently
// generated script package. Capture append must receive a copied OLB at the
// returned CaptureLibrary path; the original user library is never opened here.
func PrepareCadenceNative(batch string, parts []Part, nonce string, appendCapture bool, unitsPerInch float64) (CadenceNativePlan, error) {
	plan := CadenceNativePlan{Nonce: nonce, Entries: CadenceEntryNames(parts), Allegro: []CadenceNativeAllegro{}}
	if !regexp.MustCompile(`^[a-f0-9]{64}$`).MatchString(nonce) {
		return plan, errors.New("Cadence native nonce无效")
	}
	if unitsPerInch == 0 {
		unitsPerInch = 100
	}
	if math.IsNaN(unitsPerInch) || math.IsInf(unitsPerInch, 0) || unitsPerInch <= 0 || unitsPerInch > 1000000 {
		return plan, errors.New("Capture单位比例必须为有限正数且不超过1000000")
	}
	raw, err := os.ReadFile(filepath.Join(batch, "cadence-manifest.json"))
	if err != nil {
		return plan, err
	}
	var manifest CadenceManifest
	if err = json.Unmarshal(raw, &manifest); err != nil {
		return plan, err
	}
	put := func(rel, text string) error {
		if !cadenceBatchRelative(rel) {
			return errors.New("Cadence job路径无效")
		}
		return os.WriteFile(filepath.Join(batch, filepath.FromSlash(rel)), []byte(text), 0600)
	}
	if manifest.CaptureScript != "" {
		plan.CaptureLibrary = manifest.CaptureLibrary
		plan.CaptureBuild = "capture/native-build.tcl"
		plan.CaptureReadback = "capture/native-readback.tcl"
		plan.CaptureBuildReport = "capture/native-build.tsv"
		plan.CaptureReadbackReport = "capture/native-readback.tsv"
		body, _, e := renderCadenceCaptureMode(parts, plan.Entries, manifest.Library, nonce, appendCapture)
		if e != nil {
			return plan, e
		}
		body = captureInventoryPrelude + body
		if err = put("capture/run-config.tcl", fmt.Sprintf("set ::psCaptureUnitsPerInch %.12g\n", unitsPerInch)); err != nil {
			return plan, err
		}
		if err = put(plan.CaptureBuild, captureNativeWrap(body, nonce, "build", "native-build.tsv")); err != nil {
			return plan, err
		}
		readback := capturePrelude + captureInventoryPrelude + captureReadbackBody(manifest.Library, plan.Entries, nonce)
		if err = put(plan.CaptureReadback, captureNativeWrap(readback, nonce, "readback", "native-readback.tsv")); err != nil {
			return plan, err
		}
	}
	for i, job := range manifest.Allegro {
		if i >= len(parts) || !cadenceBatchRelative(job.Directory) {
			return plan, errors.New("Allegro job与器件不一致")
		}
		for _, p := range parts[i].Footprint.Pads {
			if !regexp.MustCompile(`^[A-Za-z0-9_.+\-]*$`).MatchString(p.Number) {
				return plan, fmt.Errorf("Allegro native回读暂不支持此焊盘编号：%q", p.Number)
			}
		}
		native := CadenceNativeAllegro{CadenceAllegroJob: job, BuildReport: job.Directory + "/native-build.tsv", ReadbackScript: job.Directory + "/native-readback.scr", ReadbackReport: job.Directory + "/native-readback.tsv", CompiledScript: job.Directory + "/native-compiled.scr", CompiledReport: job.Directory + "/native-compiled.tsv"}
		path := filepath.Join(batch, filepath.FromSlash(job.Directory), "build.il")
		skill, e := os.ReadFile(path)
		if e != nil {
			return plan, e
		}
		needle := "    unless(axlSaveDesign("
		if strings.Count(string(skill), needle) != 1 {
			return plan, errors.New("Allegro构建脚本保存点无效")
		}
		tag := fmt.Sprintf("    axlDBCreatePropDictEntry(\"PACKAGE_STUDIO_NONCE\" \"string\" \"designs\")\n    axlDBAddProp(axlDBGetDesign() list(\"PACKAGE_STUDIO_NONCE\" %s))\n", skillString(nonce))
		skillText := strings.Replace(string(skill), needle, tag+needle, 1) + "\nprocedure(psNativeBuildReport() let((r) r=outfile(\"native-build.tsv.tmp\" \"w\") fprintf(r \"protocol\\tpackage-studio.cadence/1\\nphase\\tbuild\\nnonce\\t%s\\ncomplete\\t1\\n\" axlDBGetDesign()->prop->PACKAGE_STUDIO_NONCE) close(r) unless(renameFile(\"native-build.tsv.tmp\" \"native-build.tsv\") error(\"Cannot publish complete build report\"))))\n"
		if err = os.WriteFile(path, []byte(skillText), 0600); err != nil {
			return plan, err
		}
		if err = put(job.Script, "skill load(\"build.il\")\nskill psBuildPackage()\nskill psNativeBuildReport()\nexit\n"); err != nil {
			return plan, err
		}
		if err = put(job.Directory+"/native-readback.il", allegroNativeReadback(job, nonce, false)); err != nil {
			return plan, err
		}
		if err = put(native.ReadbackScript, "skill load(\"native-readback.il\")\nskill psNativeReadback()\nexit\n"); err != nil {
			return plan, err
		}
		if err = put(job.Directory+"/native-compiled.il", allegroNativeReadback(job, nonce, true)); err != nil {
			return plan, err
		}
		if err = put(native.CompiledScript, "skill load(\"native-compiled.il\")\nskill psNativeReadback()\nexit\n"); err != nil {
			return plan, err
		}
		plan.Allegro = append(plan.Allegro, native)
	}
	return plan, nil
}

func cadenceBatchRelative(path string) bool {
	return path != "" && !filepath.IsAbs(path) && !strings.Contains(path, "\\") && filepath.ToSlash(filepath.Clean(path)) == path && path != ".." && !strings.HasPrefix(path, "../")
}

func captureNativeWrap(body, nonce, phase, report string) string {
	// Capture may forward command-line scripts to an existing GUI. This guard
	// executes before any Dbo call and rejects forwarding to another process.
	return fmt.Sprintf(`# Independent Capture process guard. Never source in an existing user session.
set psNativeDirectory [file dirname [file normalize [info script]]]
set psNativeCode [catch {
 set psUntil [expr {[clock seconds]+10}]
 while {![file exists [file join $psNativeDirectory engine.pid]]} {
  if {[clock seconds]>$psUntil} {error "Missing private-process PID handshake"}
  after 20
 }
 set psPidFile [open [file join $psNativeDirectory engine.pid] r]
 set psExpectedPID [string trim [read $psPidFile]]
 close $psPidFile
 if {![string is integer -strict $psExpectedPID] || [pid]!=$psExpectedPID} {error "Refusing script forwarded into an existing Capture process"}
%s
 set psNativeReportPath [file join $psNativeDirectory %s]
 set psNativeReport [open "$psNativeReportPath.tmp" w]
 fconfigure $psNativeReport -encoding utf-8 -translation lf
 puts $psNativeReport "protocol	package-studio.cadence/1"
 puts $psNativeReport "phase	%s"
 puts $psNativeReport "nonce	%s"
 puts $psNativeReport "pid	[pid]"
 set psVersion unknown
 if {[llength [info commands GetProductVersion]]} {set psVersion [GetProductVersion]}
 puts $psNativeReport "engine	[psHex $psVersion]"
 if {[info exists psNativeRecords]} {foreach psRecord $psNativeRecords {puts $psNativeReport $psRecord}}
 puts $psNativeReport "complete	1"
 close $psNativeReport
 file rename -force "$psNativeReportPath.tmp" $psNativeReportPath
} psNativeError]
if {$psNativeCode} {
 set psNativeErrorFile [open [file join $psNativeDirectory native-error.txt] w]
 fconfigure $psNativeErrorFile -encoding utf-8
 puts $psNativeErrorFile $psNativeError
 if {[info exists ::errorInfo]} {puts $psNativeErrorFile $::errorInfo}
 close $psNativeErrorFile
 return -code error $psNativeError
}
`, body, tclString(report), phase, nonce)
}

// Dbo signatures: Cadence Tcl/Tk Extensions Rev1.2, pp.176,196,332,585,
// 588,596,602,724; official Cadence Community OLB enumeration examples.
const captureInventoryPrelude = `
proc psHex {value} {binary scan [encoding convertto utf-8 $value] H* result; return $result}
proc psCStringValue {object method} {
 set value [DboTclHelper_sMakeCString]
 psState [$object $method $value] $method
 return [DboTclHelper_sGetConstCharPtr $value]
}
proc psPackageInventory {package status} {
 set name [psCStringValue $package GetName]
 set footprint [psCStringValue $package GetPCBFootprint]
 set devices {}
 set iterator [psObject [$package NewDevicesIter $status] $status NewDevicesIter]
 set device [$iterator NextDevice $status]
 psState $status NextDevice
 while {$device ne "NULL" && $device ne "0" && $device ne ""} {
  set position [$device GetPosition $status]
  psState $status GetPosition
  set cell [psObject [$device GetCell $status] $status GetCell]
  set symbol [psObject [$cell FindPart 0 $status] $status FindPart]
  set count [$symbol GetPinCount $status]
  psState $status GetPinCount
  set pins {}
  for {set index 0} {$index<$count} {incr index} {
   set pin [psObject [$symbol GetPin $index $status] $status GetPin]
   set pinName [psCStringValue $pin GetPinName]
   set number [DboTclHelper_sMakeCString]
   psState [$device GetPinNumber $index $number] GetPinNumber
   lappend pins [list [DboTclHelper_sGetConstCharPtr $number] $pinName]
  }
  lappend devices [list $position $pins]
  set device [$iterator NextDevice $status]
  psState $status NextDevice
 }
 delete_DboPackageDevicesIter $iterator
 return [list $name $footprint [lsort -integer -index 0 $devices]]
}
proc psLibraryInventory {library status} {
 set result {}
 set iterator [psObject [$library NewPackagesIter $status] $status NewPackagesIter]
 set package [$iterator NextPackage $status]
 psState $status NextPackage
 while {$package ne "NULL" && $package ne "0" && $package ne ""} {
  lappend result [psPackageInventory $package $status]
  set package [$iterator NextPackage $status]
  psState $status NextPackage
 }
 delete_DboLibPackagesIter $iterator
 return [lsort -ascii -index 0 $result]
}
`

func captureReadbackBody(library string, entries []string, nonce string) string {
	var b strings.Builder
	fmt.Fprintf(&b, "\nset psOutput [file join $psDirectory %s]\nset psSession [DboTclHelper_sCreateSession]\nset psStatus [DboState]\nset psLibrary [psObject [$psSession GetLib [DboTclHelper_sMakeCString $psOutput] $psStatus] $psStatus GetLib]\nset psActual [psLibraryInventory $psLibrary $psStatus]\nset psBeforeFile [open [file join $psDirectory before-inventory.tcldata] r]\nfconfigure $psBeforeFile -encoding utf-8\nset psBefore [lindex [read $psBeforeFile] 0]\nclose $psBeforeFile\n", tclString(library+".olb"))
	b.WriteString(`foreach old $psBefore {
 set found 0
 foreach actual $psActual {
  if {[lindex $actual 0] eq [lindex $old 0]} {
   if {$actual ne $old} {error "Existing package pin/footprint inventory changed"}
   set found 1
  }
 }
 if {!$found} {error "Existing package disappeared during append"}
}
set psNativeRecords [list "preserved\t[llength $psBefore]"]
`)
	for _, entry := range entries {
		fmt.Fprintf(&b, `set psPackage [psObject [$psLibrary GetPackage [DboTclHelper_sMakeCString %s] $psStatus] $psStatus GetPackage]
set psNonceValue [DboTclHelper_sMakeCString]
psState [$psPackage GetEffectivePropStringValue [DboTclHelper_sMakeCString "PackageStudioNonce"] $psNonceValue] GetNonce
set psReadNonce [DboTclHelper_sGetConstCharPtr $psNonceValue]
if {$psReadNonce ne %s} {error "Library nonce readback mismatch"}
set psPackageData [psPackageInventory $psPackage $psStatus]
lappend psNativeRecords "package\t[psHex [lindex $psPackageData 0]]\t[psHex $psReadNonce]"
foreach psDevice [lindex $psPackageData 2] {
 set psUnit [expr {[lindex $psDevice 0]+1}]
 foreach psPin [lindex $psDevice 1] {lappend psNativeRecords "pin\t[psHex [lindex $psPackageData 0]]\t$psUnit\t[psHex [lindex $psPin 0]]\t[psHex [lindex $psPin 1]]"}
}
`, tclString(entry), tclString(nonce))
	}
	b.WriteString("$psSession RemoveLib $psLibrary\nDboTclHelper_sDeleteSession $psSession\n")
	return b.String()
}

func allegroNativeReadback(job CadenceAllegroJob, nonce string, compiled bool) string {
	var b strings.Builder
	b.WriteString("; Independent native readback. Run only in the private batch process.\nprocedure(psNativeReadback()\n let((design nativeNonce r placed entry pins)\n")
	if compiled {
		b.WriteString("  unless(axlOpenDesign(?design \"placement-check.brd\" ?mode \"n\" ?noMru t) error(\"Cannot create isolated placement check\"))\n  axlSetVariable(\"padpath\" \".\")\n  axlSetVariable(\"psmpath\" \".\")\n")
		fmt.Fprintf(&b, "  placed=car(axlDBCreateSymbol(list(%s \"package\") list(0.0 0.0)))\n  unless(placed error(\"Compiled PSM cannot be loaded/placed\"))\n  entry=placed->name\n  pins=placed->pins\n  nativeNonce=%s\n  r=outfile(\"native-compiled.tsv.tmp\" \"w\")\n", skillString(job.Entry), skillString(nonce))
	} else {
		fmt.Fprintf(&b, "  unless(axlOpenDesignForBatch(%s \"wf\") error(\"Cannot reopen staged DRA\"))\n  design=axlDBGetDesign()\n  nativeNonce=design->prop->PACKAGE_STUDIO_NONCE\n  unless(nativeNonce==%s error(\"DRA nonce mismatch\"))\n  entry=axlGetDrawingName()\n  pins=design->pins\n  axlSetVariable(\"padpath\" \".\")\n", skillString(job.Entry+".dra"), skillString(nonce))
		for _, file := range job.ExpectedFiles {
			if strings.EqualFold(filepath.Ext(file), ".pad") {
				name := strings.TrimSuffix(filepath.Base(file), ".pad")
				fmt.Fprintf(&b, "  unless(axlLoadPadstack(%s) error(\"Native padstack could not be reloaded\"))\n", skillString(name))
			}
		}
		b.WriteString("  r=outfile(\"native-readback.tsv.tmp\" \"w\")\n")
	}
	phase := "readback"
	if compiled {
		phase = "readback-psm"
	}
	report := "native-readback.tsv"
	if compiled {
		report = "native-compiled.tsv"
	}
	fmt.Fprintf(&b, "  unless(r error(\"Cannot write native readback report\"))\n  fprintf(r \"protocol\\tpackage-studio.cadence/1\\nphase\\t%s\\nnonce\\t%%s\\nentry\\t%%s\\nengine\\t%%s\\n\" nativeNonce entry axlVersion('version))\n  foreach(pin pins fprintf(r \"pin\\t%%s\\n\" (pin->number || \"\")))\n  fprintf(r \"complete\\t1\\n\")\n  close(r)\n  unless(renameFile(%s %s) error(\"Cannot publish complete readback report\"))\n  t\n )\n)\n", phase, skillString(report+".tmp"), skillString(report))
	return b.String()
}
