# Cadence integration boundary

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

Allegro APIs and CLI syntax were checked against Cadence's padstack application note and Community examples. Supported pad shapes are circle, oval, rectangle and rounded rectangle, with SMD or circular through-holes. Custom pad polygons and slots fail explicitly. Solder/paste defaults match the pad outline; roundrect corners assume 15% of the short side; source masks, thermals/antipads, courtyard bounds and text blocks need target-version checks. STEP is copied independently, without automatic 3D association.

Capture APIs were checked in Cadence **OrCAD Capture Tcl/Tk Extensions, Rev.1.2**, especially pp.215,287-291,334-336,586,592-593,599,726. Tcl creates a private Dbo session and a new package/cell/part/device tree; pins have both scalar graphics and device-number mappings. Rectangles, lines, circles and polygon outlines are emitted; filled graphical areas currently retain outlines. Unit count/gate mappings are emitted but are not claimed as validated on an actual Cadence release.

The API reference does not state the physical size of the integer CPoint unit. **100 DBU/inch is an unverified compatibility default**, with `-CaptureUnitsPerInch` available to override it. Verify that a source 2.54 mm pin lead becomes one 100-mil schematic interval. Pin enums are resolved from the running Capture's own `DboValue_*` constants, never hardcoded guessed integers; unavailable enums cause an explicit failure. Passive fallbacks and collapsed power directions are recorded in export warnings. Pin names, font/number placement and source ERC distinctions need review.

Sources:

- https://community.cadence.com/cfs-file/__key/communityserver-discussions-components-files/137/MakePadstackInAllegroSkill.pdf
- https://community.cadence.com/cadence_technology_forums/pcb-design/f/allegro-x-pcb-editor/22237/smt-pad-skill
- https://community.cadence.com/cadence_technology_forums/pcb-design/f/allegro-x-scripting-skill/62614/can-skill-be-run-in-batch-mode
- https://www.ema-eda.com/wp-content/uploads/files/resources/files/OrCAD_Capture_TclTk_Extensions.pdf
- https://community.cadence.com/cadence_technology_forums/pcb-design/f/pcb-design/30973/how-to-create-new-pin-number-by-tcl-script

The generator is independently written. No inspected application binary or complete original script template is redistributed.
