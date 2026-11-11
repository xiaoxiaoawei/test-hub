# PADS Reuse 3D candidate adapter — pending real EDA acceptance

This independently written adapter calls a selected, licensed PADS installation.
It bundles no PADS executable, vendor SDK, original PackageTool implementation,
blank PCB, activation data or private C++ alignment interface. This machine has
no PADS installation. Real ImportModel, persistence, private startup, licensing,
fixture import, process exit and geometry readback remain **unverified on EDA**.
Passing simulated-engine or script-generation tests is not EDA acceptance.

## Two modes and separate engine profiles

`eda.ExportPADSModels(ctx, request, existingASCIIResult)` adds per-model evidence
without changing the native-generation status of the existing 2D export.

- `modelAssociationMode: prepare` (default) publishes an association plan,
  prepared STEP files and reproducible ASCII fixture definitions. Every model
  remains `status: prepared`, `verified: false`, `geometryVerified: false`.
  There is no CAD start or Reuse modification. This is a preparation package,
  not a proprietary library and not a promise of successful later import.
- `modelAssociationMode: native` uses `padsModelExe` and `padsReuseDirectory`.
  The deliberately narrow candidate is **PADS Classic VX2.7 / 2019.2, x86**.
  Version metadata and the native bridge version must both match 2019.2.
  PADS 9.4, older VX versions, PADS Pro and unknown architectures are refused.
  This candidate version restriction is not a compatibility certification.

`Config.Enabled` controls only the independent native **2D** library engine.
For ASCII + modern Reuse, leave that 2D switch off and explicitly select native
model association plus its modern executable and Reuse directory.

The modern engine is independent of `padsRoot`, which selects the Classic 9.4
four-file adapter. Native 2D-library generation plus native modern model
association is rejected **before** either engine is called; those independent
engines do not yet share one staged transaction. ASCII + modern Reuse is supported.

The model must have `Prepared=true` and
`PlacementConvention="step-baked-rhs-mm-v1"`: its rigid placement has already
been applied in STEP coordinates, with a right-handed Z-up physical frame.
The STEP unit definition is read by PADS; no second XYZ/rotation transform is
applied. Expected bounds are six physical millimeter values
`[minX,minY,minZ,maxX,maxY,maxZ]` from independent actual model geometry.
The default per-coordinate comparison tolerance is 0.05 mm (maximum 0.5 mm).
An unavailable model is reported separately; an unsupported native model fails.

## Isolation and actual API calls

Save and close all PADS Logic/Layout sessions before attempting native mode.
The helper refuses existing `powerpcb` or `powerlogic` processes, never kills
them and repeats this check before each owned CAD process. Only the selected
installation's executable is copied into a private batch. The original installed
dependencies stay in their installation. The selected installation's own
`Settings/default.pcb` is copied; no original tool template is redistributed.

The command line contains only `/s<generated-basic-path>`, with no document
argument before the guard. Before any library access or template open, Basic
checks its executable path, private INI nonce, and effective Reuse path. Before
accessing the native 3D bridge, its `Gui.HWND` must belong to the very same
process PID, and Classic's active document must still be our private PCB.
Basic also records its PID; it must equal the exact PID returned by Process.Start,
so a launcher cannot silently redirect evidence to a different GUI child.
There is no registered CAD application activation, ROT attachment or global
configuration fallback. A private INI/dependency incompatibility is an error.
Session forwarding and private configuration resolution still need real EDA
acceptance; the guards are requirements, not proof that every version supports them.

Each process creates its own disposable library using `Application.CreateLibrary`,
imports our own `.d/.p` with `ImportLibraryItems2`, opens its private default PCB,
and imports a one-part PADS netlist. It checks exact PT1 / part type / decal,
uses `Component.Move(0,0)` and zero orientation, then checks the actual placement.
`Application.MGCPCBApplication.ActiveDocument.System3D` must become ready.
The candidate calls `Mappings().ImportModel(step, part, decal)` and
`ExportModels(privateReuse)`, then checks the actual returned model name.
It never writes or edits `XDAlignment.dat` or XTD data itself.

Each verification starts a **separate process and a fresh private PCB**. No
ImportModel or ExportModels call occurs in that phase. The checks require:

1. `IsMapped(part,decal)` true and `IsLocallyMapped(part,decal)` false.
2. `GetModelName` equal to the precise imported STEP filename.
3. The saved package's nonempty XTD and the fresh PCB's loaded XTD have the
   same SHA-256; local mapping cache files must be absent or empty.
4. `System3D.Model.BoundingBox(2, component.UniqueId)` is finite/valid and its
   mm coordinates match the independently prepared expectation.
5. Reopening does not change any candidate Reuse bytes.

An SDK may automatically recenter an imported model. Merely baking STEP does
not establish identity import placement. Any extra centering, seating-plane
offset or other unexpected transform fails the bounding-box check. The adapter
does not infer a corrective offset or use an undocumented alignment ABI.
Native API authorization errors are returned; no token is manufactured, license
setting changed or activation bypass attempted. A version requiring a separate
Automation Licensing handshake may require a future documented adaptation.

The helper owns only its spawned process handles and a kill-on-close Windows
Job Object. Cancellation terminates that batch's children. The System3D view
may require the owned native main window. After evidence is written the helper
requests a normal owned-window close and requires the actual process to exit
with code zero. A close request failure, timeout, nonzero exit or necessary
forced kill rejects that attempt; force termination is failure cleanup only.
Save/flush behavior is still an EDA acceptance item. Fresh-process readback
must reject lost persistence.

The owned CAD is started with normal window style; its own native window may
appear during processing. The helper console stays hidden. Microsoft's
[MainWindowHandle documentation](https://learn.microsoft.com/en-us/dotnet/api/system.diagnostics.process.mainwindowhandle)
states that hidden windows can produce a zero handle, and
[CloseMainWindow](https://learn.microsoft.com/en-us/dotnet/api/system.diagnostics.process.closemainwindow)
cannot request an orderly exit without a main window. The adapter therefore
does not hide the owned CAD and then assume CloseMainWindow succeeded. It does
not call PADS' discouraged Application.Quit or drive a user's other windows.

PowerShell uses `RemoteSigned`. Downloaded files carrying Mark-of-the-Web or
enterprise execution policy may be refused. The adapter reports that failure;
it never changes system policy or removes security metadata.

## Managed Reuse append and publication

An empty Reuse, or one with unrelated files but no unknown 3D database, can be
initialized. Unknown manufacturer/third-party Reuse databases cannot be merged
without a complete trustworthy enumeration; such attempts are refused.

Successful initialization creates `package-studio-reuse.json` recording every
part/decal/model identity, prepared STEP SHA-256, expected bounds, native XTD
SHA-256, fixture definitions and a complete file-hash snapshot. Retained own
STEP files and fixture definitions live under `.package-studio`.

Append is allowed only when every preexisting file matches that manifest.
Same-name mappings are refused by default. Old and new mappings are all opened
from fresh boards and checked; old XTD bytes and expected geometry must remain
unchanged. Deleted original files, changes outside the candidate 3DModels tree,
unsupported generated file types, external edits or incomplete reports fail.
The snapshot is local integrity bookkeeping, not an authentication signature.
Do not manually edit it or use it to bless an unrelated database.

Only after all checks does one `PublishUpdates` transaction publish all Reuse
changes, managed manifest and association evidence. Compare-and-swap hashes
protect existing destinations, and transaction backups/rollback preserve the
previous files on failure. The absolute backup path is returned even on a
publication error. A failed publication never returns verified associations.
This is a recoverable multi-file transaction, not a power-loss atomic guarantee.
The final Reuse path is **not registered globally**; select it manually in PADS.

The limits are 50 newly selected parts per call, 500 managed total mappings,
4096 files / 512 MiB copied Reuse tree, and a 10-minute CAD-call context.
No background database scanning or bulk remote acquisition occurs. Engine
failures retain bounded text diagnostics; private CAD executables, INI files,
templates, native Reuse copies and STEP source files are excluded.

## Evidence and documentation boundary

Primary vendor documents establish the public 2D API and behavior:

- Mentor Graphics, *PADS Layout User's Guide and Reference Manual, PADS 9.4*,
  [public vendor-authored PDF copy](https://www.freecalypso.org/pub/CAD/PADS/pdfdocs/padslayout_ref.pdf):
  pp. 95–96 `/sXXX`, p. 2113 `CreateLibrary`, p. 2120 `OpenDocument` (failure can
  return the current document), pp. 2143–2144 `Component.Move`, p. 2159
  `ImportNetList`, pp. 2181–2182 `ImportLibraryItems2`.
- Mentor Graphics, *PADS Layout ASCII Format Specification, PADS 9.4*,
  [public vendor-authored PDF copy](https://www.freecalypso.org/pub/CAD/PADS/pdfdocs/Ppcb_ASCII.pdf),
  documents PART records and part/decal identities.
- Siemens' [MCAD collaboration article](https://blogs.sw.siemens.com/electronic-systems-design/2019/05/03/the-necessity-and-benefits-of-mcad-collaboration-part-1/)
  confirms STEP model import and Update Library for later designs.

The precise modern `System3D.Mappings` signatures, 2019.2 capability restriction,
Gui.HWND bridge ownership, enum values and persisted XTD directory conventions
were observed as **API facts** in the installed tool's readable scripts, without
executing them or copying their implementation. No complete public Siemens API
manual for these modern members was found. These are guarded candidate calls
requiring real SDK acceptance, not a claim of officially certified support.

Acceptance must use a licensed real installation: check fresh generation and
managed append; two distinct part/decal mappings; known translated/rotated
geometry; expected conflict rejection; reopen after restarting PADS; preservation
of existing user files; cancellation and rollback. Never publish a mapping as
accepted merely because this source package or its simulated tests exist.
