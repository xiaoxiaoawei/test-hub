package core

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
	"time"
)

// CadenceCandidate is an installation candidate, not a license/runtime test.
type CadenceCandidate struct {
	Product         string `json:"product"`
	Executable      string `json:"executable"`
	VersionHint     string `json:"versionHint,omitempty"`
	Source          string `json:"source"`
	RuntimeVerified bool   `json:"runtimeVerified"`
}
type CadenceAllegroJob struct {
	Entry            string   `json:"entry"`
	Directory        string   `json:"directory"`
	Script           string   `json:"script"`
	CompletionMarker string   `json:"completionMarker"`
	ExpectedFiles    []string `json:"expectedFiles"`
}
type CadenceManifest struct {
	Schema              string              `json:"schema"`
	Status              string              `json:"status"`
	Library             string              `json:"library"`
	Created             string              `json:"created"`
	Allegro             []CadenceAllegroJob `json:"allegro"`
	CaptureScript       string              `json:"captureScript,omitempty"`
	CaptureLibrary      string              `json:"captureLibrary,omitempty"`
	ExpectedNativeFiles []string            `json:"expectedNativeFiles"`
	Candidates          []CadenceCandidate  `json:"installationCandidates"`
	Warnings            []string            `json:"warnings"`
}

// ExportCadence creates an atomic, new intermediate batch. It never labels scripts
// as native .pad/.dra/.psm/.olb files or launches an unverified EDA installation.
// RUN-CADENCE.ps1 invokes an explicitly selected licensed engine and verifies its
// completion marker plus every expected native output.
func ExportCadence(parts []Part, outputDirectory, libraryName string, includeSymbol, includeFootprint bool) (ExportResult, error) {
	result := ExportResult{Files: []string{}, Warnings: []string{}}
	if len(parts) == 0 {
		return result, errors.New("请先选择需要导出的器件")
	}
	if !includeSymbol && !includeFootprint {
		return result, errors.New("请至少选择原理图或 PCB 封装")
	}
	if !filepath.IsAbs(outputDirectory) {
		return result, errors.New("请选择完整的导出目录路径")
	}
	for _, p := range parts {
		if err := ValidatePart(p, includeSymbol, includeFootprint); err != nil {
			return result, fmt.Errorf("%s：%w", p.Name, err)
		}
	}
	source, err := json.MarshalIndent(parts, "", "  ")
	if err != nil {
		return result, fmt.Errorf("器件几何无效：%w", err)
	}
	libraryName = cadenceName(libraryName)
	if err = os.MkdirAll(outputDirectory, 0755); err != nil {
		return result, err
	}
	stage, err := os.MkdirTemp(outputDirectory, ".cadence-stage-")
	if err != nil {
		return result, err
	}
	defer os.RemoveAll(stage)
	suffix := strings.TrimPrefix(filepath.Base(stage), ".cadence-stage-")
	dest := filepath.Join(outputDirectory, libraryName+"_Cadence_"+time.Now().Format("20060102_150405")+"_"+suffix)
	put := func(rel string, data []byte) error {
		path := filepath.Join(stage, rel)
		if err := os.MkdirAll(filepath.Dir(path), 0755); err != nil {
			return err
		}
		if err := os.WriteFile(path, data, 0644); err != nil {
			return err
		}
		result.Files = append(result.Files, filepath.Join(dest, rel))
		return nil
	}
	manifest := CadenceManifest{Schema: "package-studio.cadence/1", Status: "scripts-only", Library: libraryName, Created: time.Now().UTC().Format(time.RFC3339), Allegro: []CadenceAllegroJob{}, ExpectedNativeFiles: []string{}, Candidates: DetectCadenceTools()}
	warnings := []string{"当前交付为 Cadence 中间脚本包；尚未运行目标 EDA 引擎，未生成正式 .pad/.dra/.psm/.olb 库。"}
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
		warnings = append(warnings, p.Warnings...)
		warnings = append(warnings, pinMappingWarnings(p)...)
		if includeFootprint {
			skill, job, w, err := renderCadenceAllegro(p, entry)
			if err != nil {
				return result, fmt.Errorf("%s：%w", p.Name, err)
			}
			warnings = append(warnings, w...)
			rel := filepath.Join("allegro", entry)
			if err = put(filepath.Join(rel, "build.il"), []byte(skill)); err != nil {
				return result, err
			}
			if err = put(filepath.Join(rel, "build.scr"), []byte("skill load(\"build.il\")\nskill psBuildPackage()\nexit\n")); err != nil {
				return result, err
			}
			manifest.Allegro = append(manifest.Allegro, job)
			manifest.ExpectedNativeFiles = append(manifest.ExpectedNativeFiles, job.ExpectedFiles...)
		}
		if includeFootprint && p.Model != nil && p.Model.Path != "" {
			ext := strings.ToLower(filepath.Ext(p.Model.Path))
			if ext != ".step" && ext != ".stp" {
				warnings = append(warnings, "Cadence 模型复制仅支持 STEP/STP："+p.Name)
				continue
			}
			src, err := os.Open(p.Model.Path)
			if err != nil {
				return result, err
			}
			info, err := src.Stat()
			if err != nil || !info.Mode().IsRegular() || info.Size() > 128*1024*1024 {
				src.Close()
				return result, errors.New("STEP 模型必须为不超过128 MiB的普通文件")
			}
			rel := filepath.Join("step", entry+ext)
			if err = os.MkdirAll(filepath.Dir(filepath.Join(stage, rel)), 0755); err != nil {
				src.Close()
				return result, err
			}
			dst, err := os.Create(filepath.Join(stage, rel))
			if err != nil {
				src.Close()
				return result, err
			}
			_, cpErr := io.Copy(dst, src)
			closeErr := dst.Close()
			src.Close()
			if cpErr != nil {
				return result, cpErr
			}
			if closeErr != nil {
				return result, closeErr
			}
			result.Files = append(result.Files, filepath.Join(dest, rel))
			warnings = append(warnings, "STEP 已独立复制；尚未自动关联 Allegro 3D 模型，请在目标软件中手动关联并核对朝向。")
		}
	}
	if includeSymbol {
		script, w, err := renderCadenceCapture(parts, entries, libraryName)
		if err != nil {
			return result, err
		}
		warnings = append(warnings, w...)
		if script != "" {
			manifest.CaptureScript = "capture/build.tcl"
			manifest.CaptureLibrary = "capture/" + libraryName + ".olb"
			manifest.ExpectedNativeFiles = append(manifest.ExpectedNativeFiles, manifest.CaptureLibrary)
			if err = put(filepath.Join("capture", "build.tcl"), []byte(script)); err != nil {
				return result, err
			}
		}
	}
	if len(manifest.Candidates) == 0 {
		warnings = append(warnings, "未发现标准安装位置或 PATH 中的 Cadence 工具；请在已安装并授权的目标机执行脚本包。")
	}
	warnings = uniqueCadenceWarnings(warnings)
	manifest.Warnings = warnings
	m, err := json.MarshalIndent(manifest, "", "  ")
	if err != nil {
		return result, err
	}
	for rel, data := range map[string][]byte{"cadence-manifest.json": m, "parts.source.json": source, "RUN-CADENCE.ps1": []byte(cadenceRunner), "README-CADENCE.txt": []byte(cadenceReadme)} {
		if err = put(rel, data); err != nil {
			return result, err
		}
	}
	if err = os.Rename(stage, dest); err != nil {
		return result, err
	}
	result.Directory = dest
	result.Warnings = warnings
	return result, nil
}

func cadenceName(s string) string {
	raw := s
	var b strings.Builder
	for _, r := range strings.TrimSpace(s) {
		if r >= 'a' && r <= 'z' || r >= 'A' && r <= 'Z' || r >= '0' && r <= '9' || r == '_' {
			b.WriteRune(r)
		} else {
			b.WriteByte('_')
		}
	}
	s = strings.Trim(b.String(), "_")
	if s == "" {
		s = "Component"
	}
	if s[0] >= '0' && s[0] <= '9' {
		s = "C_" + s
	}
	if len(s) > 48 {
		s = s[:48]
	}
	sum := sha256.Sum256([]byte(raw))
	return s + "_" + hex.EncodeToString(sum[:4])
}
func uniqueCadenceWarnings(in []string) []string {
	out := []string{}
	seen := map[string]bool{}
	for _, s := range in {
		if s != "" && !seen[s] {
			seen[s] = true
			out = append(out, s)
		}
	}
	return out
}

const cadenceReadme = `Cadence export scripts - Package Studio

STATUS: SCRIPTS ONLY. No native Cadence binary library is supplied by generation.
The expected files in cadence-manifest.json are targets, not existing outputs.

1. Copy this entire batch to a machine with the intended, licensed Cadence release.
   Use an ASCII-only local path if the release cannot handle Unicode paths.
2. Review parts.source.json, the generated SKILL/Tcl, and all warnings.
3. Run PowerShell with explicit tool paths, e.g.:
   .\RUN-CADENCE.ps1 -AllegroExe 'C:\Cadence\SPB_17.4\tools\bin\allegro.exe' -AllegroProduct 'Allegro_performance'
   Add -CaptureExe when capture/build.tcl is present and a matching Capture
   Tcl API is available. Capture can remain open after the script finishes.
   Capture defaults to 100 DBU/inch as an UNVERIFIED compatibility scale, because
   the API reference does not specify this physical unit. Verify a 2.54 mm pin
   lead becomes one 100-mil schematic grid interval. Override using
   -CaptureUnitsPerInch <verified-value> if this release uses another scale.
4. Each Allegro job is a fresh isolated package drawing. Do not load build.il into
   an unsaved board or a working user design; use the batch command/new session.
5. The runner requires a completion marker AND all nonempty expected files.
   Process exit alone, a generated script, or an executable path is not success.
6. Add each allegro/<entry> folder to Allegro padpath and psmpath. Open the .dra,
   place the .psm into a NEW test board, and check pin numbers, pad sizes, drills,
   solder/paste openings, silk, placement bounds and copper layers. Run DRC.
7. Open the generated .olb in Capture and place every unit into a NEW design;
   inspect pin endpoints, numbering, names, electrical types and PCB Footprint.
8. STEP files under step/ are independent copies. Link/align them in Allegro and
   verify Pin 1, rotation, height and model scale. No automated 3D association.

Source evidence used for the independently written generator:
Cadence MakePadstackInAllegroSkill.pdf (2018), hosted by Cadence Community.
https://community.cadence.com/cfs-file/__key/communityserver-discussions-components-files/137/MakePadstackInAllegroSkill.pdf
https://community.cadence.com/cadence_technology_forums/pcb-design/f/allegro-x-pcb-editor/22237/smt-pad-skill
https://community.cadence.com/cadence_technology_forums/pcb-design/f/allegro-x-scripting-skill/59148/regarding-skill-commands
https://community.cadence.com/cadence_technology_forums/pcb-design/f/allegro-x-scripting-skill/62614/can-skill-be-run-in-batch-mode
Capture Dbo signatures: Cadence OrCAD Capture Tcl/Tk Extensions Rev 1.2, pages
215,287-291,334-336,586,592-593,599,726 (hosted by Cadence distributor EMA).
https://www.ema-eda.com/wp-content/uploads/files/resources/files/OrCAD_Capture_TclTk_Extensions.pdf
The generator creates a private Dbo session, package/cell/part/device objects,
scalar pins plus device pin-number mappings, geometry, then SavePackageAll/SaveLib.
Pin enum integers are NOT hardcoded; the target Capture must expose matching
DboValue_* constants, otherwise the script stops and writes capture-error.txt.
Capture filled symbol shapes currently retain outlines; multiunit gate mapping,
font/number placement, source ERC distinctions and physical scale need validation.
API names and argument facts were also checked against static constants in the
supplied application. None of its binaries or complete script templates are copied.
The current development host had no verified Cadence runtime; native validation
must therefore be completed on the target Cadence release.
`

const cadenceRunner = `param([string]$AllegroExe='',[string]$AllegroProduct='',[string]$CaptureExe='',[double]$CaptureUnitsPerInch=100.0,[int]$CaptureTimeoutSeconds=180)
$ErrorActionPreference='Stop'
$root=$PSScriptRoot
$manifest=Get-Content -LiteralPath (Join-Path $root 'cadence-manifest.json') -Raw | ConvertFrom-Json
if (@($manifest.expectedNativeFiles).Count -eq 0) {throw 'This package has no supported native-generation job. Read README-CADENCE.txt and the manifest warnings.'}
if ($manifest.captureScript -and (-not $CaptureExe -or -not (Test-Path -LiteralPath $CaptureExe -PathType Leaf))) {throw 'Provide matching, licensed Capture with -CaptureExe, or source capture/build.tcl in Capture.'}
foreach($expected in $manifest.expectedNativeFiles){if(Test-Path -LiteralPath (Join-Path $root $expected)){throw 'Existing native output found. Use a fresh batch; no overwrite is allowed.'}}
if ($manifest.allegro.Count -gt 0) {
 if (-not $AllegroExe -or -not (Test-Path -LiteralPath $AllegroExe -PathType Leaf)) {throw 'Provide a real, licensed Allegro executable with -AllegroExe.'}
 foreach($job in $manifest.allegro) {
  foreach($expected in $job.expectedFiles) {if(Test-Path -LiteralPath (Join-Path $root $expected)){throw 'Existing native output found. Use a fresh batch; no overwrite is allowed.'}}
  $dir=Join-Path $root $job.directory
  $argsList=@('-nograph','-s','build.scr');if($AllegroProduct){$argsList=@('-product',$AllegroProduct)+$argsList}
  Push-Location -LiteralPath $dir
  try {& $AllegroExe @argsList 2>&1 | Tee-Object -FilePath (Join-Path $dir 'engine.log');if($LASTEXITCODE -ne 0){throw ('Allegro exit code '+$LASTEXITCODE)}} finally {Pop-Location}
  if(-not (Test-Path -LiteralPath (Join-Path $root $job.completionMarker))){throw 'Allegro did not report completion. Read engine.log and allegro.jrl.'}
  foreach($expected in $job.expectedFiles){$file=Get-Item -LiteralPath (Join-Path $root $expected) -ErrorAction Stop;if($file.Length -le 0){throw ('Empty native file: '+$expected)}}
 }
}
if ($manifest.captureScript) {
 if(-not $CaptureExe -or -not (Test-Path -LiteralPath $CaptureExe -PathType Leaf)){throw 'Provide matching, licensed Capture with -CaptureExe, or source capture/build.tcl in Capture.'}
 $script=Join-Path $root $manifest.captureScript
 $marker=Join-Path $root 'capture/capture-complete.txt'
 if(Test-Path -LiteralPath (Join-Path $root $manifest.captureLibrary)){throw 'Capture output already exists; use a fresh batch.'}
 # Launch a new tool invocation; never kill a pre-existing Capture process.
 if($CaptureUnitsPerInch -le 0 -or [Double]::IsNaN($CaptureUnitsPerInch) -or [Double]::IsInfinity($CaptureUnitsPerInch)){throw 'CaptureUnitsPerInch must be finite and positive.'}
 $scale=$CaptureUnitsPerInch.ToString('R',[Globalization.CultureInfo]::InvariantCulture)
 [IO.File]::WriteAllText((Join-Path $root 'capture/run-config.tcl'),('set ::psCaptureUnitsPerInch '+$scale+[Environment]::NewLine),[Text.UTF8Encoding]::new($false))
 $captureArgs=@('-product="OrCAD Capture"',('"'+$script+'"'))
 Start-Process -FilePath $CaptureExe -ArgumentList $captureArgs -WorkingDirectory (Join-Path $root 'capture') -WindowStyle Hidden | Out-Null
 $until=(Get-Date).AddSeconds($CaptureTimeoutSeconds)
 while(-not(Test-Path -LiteralPath $marker)){if(Test-Path -LiteralPath (Join-Path $root 'capture/capture-error.txt')){throw (Get-Content -LiteralPath (Join-Path $root 'capture/capture-error.txt') -Raw)};if((Get-Date) -gt $until){throw 'Capture timed out without completion. Inspect capture/build.tcl and the Capture session log.'};Start-Sleep -Milliseconds 500}
 $olb=Get-Item -LiteralPath (Join-Path $root $manifest.captureLibrary) -ErrorAction Stop;if($olb.Length -le 0){throw 'Capture OLB output is empty.'}
}
$native=@($manifest.expectedNativeFiles | ForEach-Object{Join-Path $root $_})
@{status='engine-generated';files=$native;placementVerified=$false;note='Native output presence verified; perform placement/ERC/DRC in Cadence.'} | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $root 'native-result.json') -Encoding UTF8
Write-Output 'Native output files exist. Perform target-version placement and ERC/DRC checks.'
`
