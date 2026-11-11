# Cadence integration boundary

Version 0.5 adds `eda.ExportCadence(ctx, eda.Request)` and `eda.DetectCadence()`.
The application invokes this native path only when local EDA integration is
enabled. A missing, invalid or unlicensed engine is an error; native mode never
silently reports a script package as a generated library. This development host
has **no Cadence installation**. The native integration is implemented and its
protocol/transaction tests pass; actual Capture/Allegro version compatibility,
license checkout and geometry still require testing on an installed target EDA.

The original portable script-package path remains available independently:

The portable Go entry points are `core.ExportCadence(parts, absoluteParentDirectory, libraryName, includeSymbol, includeFootprint)` and `core.DetectCadenceTools()`.

Export writes a fresh batch atomically. The batch contains `parts.source.json`, a manifest, replay/SKILL per Allegro component, Capture Dbo Tcl when selected, and a PowerShell runner. `ExportResult.Files` lists only files actually written. Expected `.pad/.dra/.psm/.olb` files appear separately in the manifest and are **not** supplied or reported as generated until the target engine produces them.

Detection checks PATH, explicit Cadence environment roots, common installation folders, and 32/64-bit Windows uninstall/Cadence registry keys. Every candidate has `runtimeVerified: false`; registry data and file presence do not establish a working license or compatible engine. The current host has no detected/verified Cadence engine.

From the repository root, create a demonstration batch or inspect candidates:

```powershell
go run helpers/cadence/smoke_export.go -detect
go run helpers/cadence/smoke_export.go -out C:/CadenceValidation
go run helpers/cadence/smoke_export.go -input parts.json -out C:/CadenceValidation -library MyParts
```

On the intended licensed Cadence machine, use the generated `RUN-CADENCE.ps1` with explicit `-AllegroExe`, optional `-AllegroProduct` matching the installed license, and `-CaptureExe`. Its completion markers and nonempty-file checks establish engine generation, not correct placement. Then place every footprint/gate into a new target-tool test design and run ERC/DRC. Do not load generated SKILL into an unsaved working board.

Allegro APIs and CLI syntax were checked against Cadence's padstack application note and Community examples. Supported pad shapes are circle, oval, rectangle and rounded rectangle, with SMD or circular through-holes. Custom pad polygons and slots fail explicitly. Solder/paste defaults match the pad outline; roundrect corners assume 15% of the short side; source masks, thermals/antipads, courtyard bounds and text blocks need target-version checks. Version 0.6 adds the guarded STEP association path described below.

Capture APIs were checked in Cadence **OrCAD Capture Tcl/Tk Extensions, Rev.1.2**, especially pp.215,287-291,334-336,586,592-593,599,726. Tcl creates a private Dbo session and a new package/cell/part/device tree; pins have both scalar graphics and device-number mappings. Rectangles, lines, circles and polygon outlines are emitted; filled graphical areas currently retain outlines. Unit count/gate mappings are emitted but are not claimed as validated on an actual Cadence release.

The API reference does not state the physical size of the integer CPoint unit. **100 DBU/inch is an unverified compatibility default**, with `-CaptureUnitsPerInch` available to override it. Verify that a source 2.54 mm pin lead becomes one 100-mil schematic interval. Pin enums are resolved from the running Capture's own `DboValue_*` constants, never hardcoded guessed integers; unavailable enums cause an explicit failure. Passive fallbacks and collapsed power directions are recorded in export warnings. Pin names, font/number placement and source ERC distinctions need review.

Sources:

- https://community.cadence.com/cfs-file/__key/communityserver-discussions-components-files/137/MakePadstackInAllegroSkill.pdf
- https://community.cadence.com/cadence_technology_forums/pcb-design/f/allegro-x-pcb-editor/22237/smt-pad-skill
- https://community.cadence.com/cadence_technology_forums/pcb-design/f/allegro-x-scripting-skill/62614/can-skill-be-run-in-batch-mode
- https://www.ema-eda.com/wp-content/uploads/files/resources/files/OrCAD_Capture_TclTk_Extensions.pdf
- https://community.cadence.com/cadence_technology_forums/pcb-design/f/pcb-design/30973/how-to-create-new-pin-number-by-tcl-script

The generator is independently written. No inspected application binary or complete original script template is redistributed.

## Native generation and append

- Configure an actual `Capture.exe` for symbols and/or `allegro.exe` for
  footprints. Discovery's `ready` means a regular Windows x86/x64 PE candidate
  with the expected Capture/Allegro/pcb_editor executable basename;
  `runtimeVerified` remains false until a real conversion and readback succeed.
  An optional Allegro product selects the user's installed license product.
- Close the destination library in EDA before appending. Capture append copies
  the existing `.olb` into a private staging directory, records its SHA-256,
  and opens only that copy with `DboSession.GetLib`. It rejects an existing
  package name before adding anything. New mode uses `CreateLib`.
- Capture scripts create their own Dbo session. Before any Dbo operation they
  compare Tcl `[pid]` with the PID of the newly spawned child. If Capture forwards
  the script into an existing GUI, this guard fails; no user's open library or
  project is accessed. `-i` points to the staging configuration directory.
- Saving is followed by a separate new Capture process/session that reopens the
  staged OLB. It reads package names, each device/gate, pin names and physical
  pin numbers, and the stored `PackageStudioNonce` property. Append also compares
  the prior packages' complete pin/footprint inventories with the reopened copy.
  This inventory check is not an ERC, graphical equivalence test or vendor
  qualification of all legacy OLB contents.
- Allegro generates independent `.pad/.dra/.psm` files in staging. Separate
  processes reopen the DRA, read its stored nonce and pin numbers, reload the
  padstacks, then load/place the compiled PSM in a fresh sandbox drawing and read
  its name/pins. Existing Allegro target filenames are rejected by default.
  Conflicting files are never overwritten implicitly.
- Every report is written to a temporary file, closed, then renamed to its
  final name. The host checks the protocol version, phase, random nonce,
  Capture process ID, names and pin mappings; a marker, exit code or nonempty
  native file alone is insufficient. Per-process timeouts are 180 seconds,
  with a ten-minute total operation bound. Cancellation stops only the process
  handle created by this operation, never a PID read from an EDA report.
  A two-second `WaitDelay` also bounds inherited log pipes; late logs can be
  discarded without waiting on or stopping an unrelated process. Natural
  nonzero engine exits remain errors even after a report has appeared.
- Only after all selected engines pass does the shared `PublishUpdates`
  transaction publish the family. Capture replacement requires the original
  SHA-256 to still match; existing files are backed up in the fresh batch's
  `backups` directory. Publication errors use the shared rollback mechanism.
  Backups are retained for recovery. This is not power-loss atomicity across
  several files.
- The returned `nativeGenerated/runtimeVerified` flags mean generation and the
  automated engine readback completed for this invocation. The result retains a
  warning that target-version placement/ERC/DRC, Capture physical units and
  native append compatibility have not been qualified on the development host.
  `cadence-native-result.json` records the result and actual paths. `evidence`
  contains source data, generated scripts, logs and readback reports; these are
  diagnostic copies, not a standalone rerunnable append package.
- Failures retain at most 32 MiB of text diagnostics (4 MiB per file) under
  `CadenceFailed_*`, and the error names that directory. Existing OLB copies and
  native binary files are excluded from diagnostics; staging is removed.
  STEP files are omitted from failure diagnostics. Model association failures
  do not publish either the native library or its source model.

Native tests use explicitly labelled **fakeRunner fixtures**, never original
software binaries and never a real user EDA session:

```powershell
go test ./internal/eda ./internal/core -run 'Cadence|Capture' -count=1
```

They cover new/append publication, original-file backups and concurrent changes,
preflight engine validation, filename conflicts, malformed/incomplete reports,
wrong nonces/PIDs/names/pins, compiled PSM readback rejection, bounded child
processes, diagnostic retention and Tcl source completeness. These tests do not
establish that Cadence is installed or that native EDA execution passed here.

Additional primary API references used for the new readback/append path:

- [Cadence Capture Tcl/Tk Extensions Rev.1.2](https://www.ema-eda.com/wp-content/uploads/files/resources/files/OrCAD_Capture_TclTk_Extensions.pdf), pp.176 (`GetLib`),196 (`PackageExists`/`GetPackage`),332 (`GetPinCount`),585 (`FindPart`),588 (`GetPin`),596 (`GetDevice`),602 (`GetPinName`),724 (`GetPinNumber`).
- [Cadence OLB pin-name/number enumeration](https://community.cadence.com/cadence_technology_forums/pcb-design/f/allegro-x-scripting-tcl/65760/how-do-i-parse-all-the-graphical-entities-of-a-schematic-symbol-from-an-olb/1407807).
- [Cadence drawing-level user properties](https://community.cadence.com/cadence_technology_forums/pcb-design/f/allegro-x-pcb-editor/34882/accessing-symbol-user-defined-properties-in-a-board-file).
- [Cadence design property readback](https://community.cadence.com/cadence_technology_forums/pcb-design/f/allegro-x-pcb-editor/35392/accessing-properties-in-skill-in-the-symbol-editor).
- [Cadence batch drawing reopen](https://community.cadence.com/cadence_technology_forums/pcb-design/f/allegro-x-scripting-skill/42048/board-file-use/1361675).
- [Cadence typed package-symbol placement](https://community.cadence.com/cadence_technology_forums/pcb-design/f/allegro-x-pcb-editor/7975/axldbcreatesymdefskeleton).

## STEP association in 0.6

The shared model preparer supplies `Model.Prepared=true`,
`PlacementConvention="step-baked-rhs-mm-v1"` and a `Path` to a right-handed,
Z-up STEP whose physical placement is already baked in millimetres. The STEP's
own SI unit may remain millimetres or metres; its declared unit is preserved
by the shared preparer. Empty, legacy `easyeda-std-kicad-v1` or other placement
conventions are rejected even when `Prepared=true`. Cadence never applies the
legacy `X/Y/Z` or `RotationX/Y/Z` fields again. An unprepared or invalid STEP
fails explicitly. The file is copied alongside its own Allegro job using a
safe entry name plus a SHA-256 suffix. The full hash and expected source
bounding box are recorded in `ModelAssociations`.

The portable `core.ExportCadence` path writes separate `<entry>.step-map.il`
and `<entry>.step-map.scr` identity-mapping jobs beside the STEP. Its 2D
`build.il` does not execute mapping. Its status is `prepared`, `verified=false`, and
`geometryVerified=false`: no EDA was run. Generated scripts use the current
process's `steppath="."`; they never edit a global Cadence environment file.
The standalone PowerShell runner's marker/file checks still establish only
generation. Use the application's native path for the independent readback
protocol below.

`ModelAssociationMode="prepare"` (also the default) retains those jobs without
executing them, including when native 2D generation is enabled. The 2D engine
still generates/reopens DRA/PSM, but it does not call `axlStepSet`, require model
properties or inspect facets. STEP and separate jobs are published alongside
the 2D outputs and remain `prepared`, `verified=false`. An explicit later manual
invocation must run the prepared `.scr` in a **new** batch process beside its
DRA/STEP; it does not update the application's result to verified automatically.

With local native integration enabled and
`ModelAssociationMode="native"`, a requested model must be associated
successfully; it cannot silently become an independent attachment. This path
injects identity mapping before the 2D drawing's save/compile and requires all
of the following before publication:

1. `axlStepSet('primary nil filename mapping)` succeeds in the newly created
   package drawing. The six `offset_x/y/z` and `rotation_x/y/z` values are zero.
2. A new engine invocation reopens the DRA and reads the persisted
   `PKGDEF_STEP_FILE` and `PKGDEF_STEP_TRANSFORMATION` properties. The host checks
   the exact safe filename (Windows case insensitive), millimetre units and all
   six zero mapping components.
3. Another new invocation places the compiled PSM in a fresh sandbox drawing.
   It reads the same properties from `placed->symdef`, and requires an actual
   `STEP3D_<entry>` facet attachment from `axlGetAllAttachmentNames()`. A true
   return from `axlStepSet` alone is not proof that the source was read.
4. The source STEP hash still matches. STEP and `.pad/.dra/.psm`, together with
   any selected Capture OLB, enter one `PublishUpdates` transaction. Append
   rejects any existing native or STEP destination filename before launching
   an engine. A failed model readback leaves the original OLB and Allegro
   directory untouched.

Only that successful native path sets `status="associated", verified=true`.
This is **association and facet readback**, not a 3D geometry test. We have no
verified Cadence API here that returns the imported geometry's true 3D bounding
box. `actualBBoxMm` therefore remains absent and `geometryVerified=false`;
the result explains that Pin 1, height, handedness, dimensions and target-version
rendering still need an installed EDA check. A source bounding box is never
reported as an engine-measured one. This development host has no Cadence, so
all native execution and compatibility remain pending real EDA validation.

The implementation targets releases exposing `axlStepSet` and the facet query.
It probes the APIs rather than claiming a tested range from version strings.
Allegro 16.6/hotfixes lacking these APIs fail explicitly. Cadence's legacy STEP
mapping UI and its historic unsupported-prototype environment switch are not
automatically driven or enabled. Recent Allegro X/3DX storage may differ and
requires target-release validation. Missing or differently named facet data
causes an error with retained diagnostics, not a success downgrade. Capture
OLB by itself cannot carry a footprint symdef STEP mapping; requesting a model
with only a Capture export is rejected. Parts without a model can be marked
`unavailable` without fabricating one.

Research evidence and limits:

- [Cadence footprint-library property readback](https://community.cadence.com/cadence_technology_forums/pcb-design/f/allegro-x-pcb-editor/47314/get-info-from-all-footprints-in-library?pifragment-3162=1) documents `symdef->prop->PKGDEF_STEP_FILE` and DRA-level extraction.
- [Cadence mapping-property discussion](https://community.cadence.com/cadence_technology_forums/pcb-design/f/allegro-x-pcb-editor/65531/3d-model-doesn-t-get-exported-with-footprint-library) distinguishes saved mapping/facets from the external original STEP and newer 3DX storage.
- [Cadence schematic versus footprint mapping](https://community.cadence.com/cadence_technology_forums/pcb-design/f/allegro-x-capture-cis/52580/step-model-mapping-using-database-entry) explains why schematic/component properties do not substitute for the footprint symdef mapping.
- [Cadence legacy 16.6 mapping](https://community.cadence.com/cadence_technology_forums/pcb-design/f/pcb-design/26970/step-package-mapping) records the hotfix-dependent prototype boundary.
- The Set argument order and six field names were independently read as short API facts from the supplied program's static constants, without executing it or copying its templates. Cadence's 17.4 SKILL reference index lists Set/Get, but an accessible full official signature was not obtained. [A public author's own 17.4 experiments](https://github.com/daveyJ-sgs/Cadence-MCP-SPB17/blob/main/bridge/allegro/step_mapping.py) corroborate the Set call shape and facet requirement; they are not Cadence qualification. That author's Get example is not used by this implementation. Reopened persisted property readback avoids inventing its signature.

The automated source/protocol fixtures cover identity mapping, escaping, hash
publication in new/append mode, STEP-only destination conflicts, missing or
duplicate readback records, wrong model/units/transform/facet and a changed
source model. They deliberately assert `geometryVerified=false`. Fixture STEP
text and fakeRunner native files are labelled test data and are never shipped
as a usable CAD model or evidence that Cadence ran here.
