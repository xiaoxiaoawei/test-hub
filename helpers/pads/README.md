# PADS native adapter — pending real EDA validation

This is independently written Package Studio integration code. No executable,
library, script implementation or activation data from PackageTool is bundled.
No PADS SDK is redistributed. A user must select their own licensed installation.

**Status:** this machine has no PADS installation. The Go orchestration, rollback
boundary, conflict rules and helper protocol have simulated-engine tests. The
actual PADS ABI, private startup, library imports and saved-library readback have
not been accepted on a real installation. Do not call this a verified PADS release.

Save and close every PADS Layout/Logic instance before attempting native export.
The helper checks this before preparation and again before each CAD start, and
refuses a busy machine without terminating those instances. No design filename
is passed on the command line before the Basic guards. Session forwarding and
private startup behavior remain unverified; a guard failure never falls back to
the registered application or a current user's document.

## Deliberately narrow engine scope

The candidate adapter accepts **PADS Classic 9.4, x86** metadata only, using
32-bit Windows PowerShell. It does not assume VX, PADS Pro or 64-bit compatibility.
The selected `SDD_HOME` must contain `Programs/PadsCLibInt.dll`, the selected
`powerpcb.exe`/`powerlogic.exe`, their adjacent INI files, and the corresponding
`Settings/default.pcb`/`default.sch`. Native dependencies must remain installed.
If any API, type library, dependency or private-configuration check fails, export
fails. There is no fallback into a user's running CAD or global library list.

The original Mentor **PADS Layout User's Guide and Reference Manual, PADS 9.4**
is the API reference ([public copy](https://www.freecalypso.org/pub/CAD/PADS/pdfdocs/padslayout_ref.pdf)):

- Pages 95–96: design filename and `/sXXX` startup argument.
- Page 1619: item categories: part types 0, decals 1, logic drawings 2, drawings 3.
- Pages 1641, 2180–2182: `Libraries`, `GetLibraryItems`, `ImportLibraryItems2`.

The selected DLL's standard COM factory is used solely to invoke its actual
`Pads_CreateLibraryFile` entry through IDispatch. That method name was observed
in the installed tool's plaintext SDK helper. The COM bridge here is independent
code using standard Windows APIs. It neither registers a server nor bypasses
PADS authorization. Its precise 9.4 type-library behavior still needs acceptance.

Private INI lookup and Logic's startup behavior are **runtime guard requirements,
not documented support promises**. A copied executable can lack adjacent
dependencies or require its original installation environment. In those cases the
adapter must report unsupported startup. Do not relax the guards or silently use
the normally registered application to make an unsupported installation pass.

## Publication boundary

`eda.ExportPADS(ctx, Request)` uses `core.ExportPADS` for genuine `.d/.c/.p`
interchange files and exact expected item names. In append mode the original
four-file set is copied and hashed first. The helper receives only staging paths.

1. The SDK creates a new four-file set or imports into staged copies.
2. Exact name conflicts are refused. Original item inventories must be preserved.
3. A separate helper/CAD process reopens the saved set. Names and counts must
   equal the conversion inventory. Readback must not change any library bytes.
4. Only then does `PublishUpdates` back up and replace the selected destination
   files as a transaction, checking that the originals still have their old hashes.

`NativeGenerated` and `RuntimeVerified` become true only after those engine calls
and publication succeed. A fake/empty/missing output, stale report, refused import
or failed readback is an error. Renaming ASCII to `.pt9` is never a conversion.
The adapter validates names and counts, not electrical or manufacturing correctness.
The result retains a pending real-EDA acceptance warning even after automated checks.

Temporary copies of the selected installed executables are local batch working
files, not product deliverables. The Basic script checks its executable and private
INI nonce before any library call. The helper owns a Windows kill-on-close Job
Object so cancellation also terminates its child CAD process, without searching
for or killing other CAD instances. Only the staging directory is cleaned up.

Completed append backups remain at the returned `BackupDirectory`. On publication
failure the error and result also retain that absolute path for recovery. Keep the
four native extensions together. Register the final path manually in PADS; the
adapter does not install a permanent library-list entry.

Failed engine attempts retain at most 64 bounded text files (32 MiB total) in the
returned `PADSFailed_...` directory: interchange inputs, requests, responses,
generated Basic and inventory/log evidence. Copied CAD executables, native
library copies, templates and INI settings are excluded. No diagnostic is uploaded.
The temporary SDK workspace itself is removed.

The helper closes only its owned process after Basic completes. Its exact
flush/exit behavior and whether a version resolves `common/config.ini` relative
to the private executable or original `SDD_HOME` still need real SDK acceptance.
Independent reopen and unchanged-file hash checks must reject missing or changed
data; they do not justify broadening the version scope.

The launcher uses PowerShell `RemoteSigned`. Downloaded ZIP files can carry a
Windows Mark-of-the-Web, and enterprise/system policy may refuse an unsigned
helper. In that case native export reports an error. The adapter does not change
execution policy, remove security metadata or modify system settings to bypass it.

STEP files remain independent attachments. **Reuse/3DModels association is not
implemented** and is never inferred from the existence of a STEP file.

## Protocol and acceptance checks

`native.ps1 -Request <absolute-json-path>` accepts protocol 1, a random batch
nonce, `convert` or `verify`, selected SDK hashes, staging bases, interchange
inputs and expected inventories. It writes a bounded JSON evidence report to the
requested staging response path. The Go caller independently checks the report,
all four saved files and their hashes before publication. Helpers are resolved
beside the app under `adapters/pads`, with `helpers/pads` as a development fallback.

Before expanding version support, test on a licensed installation using an empty
disposable library: new symbol/footprint, symbol-only, footprint-only, shared and
separate append destinations, same-name refusal, cancellation and failed readback.
Verify original hashes and backups. Place each resulting part in Logic and Layout,
save/reopen, and inspect pin mapping, multi-unit symbols, pad geometry and layers.
Compare existing native items before/after. Only after this acceptance should a new
version's metadata be added to the allowlist. No such real SDK test has run here.
