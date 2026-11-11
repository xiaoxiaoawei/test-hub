# Pure Basic generation; no SDK loading, process startup or filesystem writes.
function BasicString([string]$value) {
 if($value.Contains("`r") -or $value.Contains("`n") -or $value.Contains([char]0)) {throw 'Invalid Basic string.'}
 return '"'+$value.Replace('"','""')+'"'
}

function New-StudioPADSBasic($Job,$Library,[string]$Role,[bool]$First,[string]$PrivateExe,[string]$Record,[string]$Base) {
 $builder=New-Object Text.StringBuilder
 [void]$builder.AppendLine('Declare Function GetCurrentProcessId Lib "kernel32" () As Long')
 [void]$builder.AppendLine('Sub DumpItems(ByVal lib As Object, ByVal label As String, ByVal channel As Integer)')
 [void]$builder.AppendLine('  Dim kind As Integer, entry As Object, collection As Object, counted As Long')
 [void]$builder.AppendLine('  For kind = 0 To 3')
 [void]$builder.AppendLine('    Set collection = lib.GetLibraryItems(kind, "*")')
 [void]$builder.AppendLine('    counted = 0')
 [void]$builder.AppendLine('    For Each entry In collection')
 [void]$builder.AppendLine('      Print #channel, label & "|" & CStr(kind) & "|" & entry.Name')
 [void]$builder.AppendLine('      counted = counted + 1')
 [void]$builder.AppendLine('    Next entry')
 [void]$builder.AppendLine('    If counted <> collection.Count Then Err.Raise 2006, , "SDK item enumeration count mismatch"')
 [void]$builder.AppendLine('  Next kind')
 [void]$builder.AppendLine('End Sub')
 [void]$builder.AppendLine('Sub Main')
 [void]$builder.AppendLine('  Dim lib As Object, items As Object, stream As Integer, full As String')
 [void]$builder.AppendLine('  On Error GoTo Failed')
 [void]$builder.AppendLine(('  If Application.GetConfigParamString("PackageStudioNative", "Nonce", "") <> '+(BasicString $job.nonce)+' Then Err.Raise 2001, , "Private INI guard failed; no library access"'))
 [void]$builder.AppendLine(('  If LCase(Application.FullName) <> LCase('+(BasicString $privateExe)+') Then Err.Raise 2002, , "Unexpected CAD executable"'))
 [void]$builder.AppendLine('  stream = FreeFile')
 [void]$builder.AppendLine(('  Open '+(BasicString $record)+' For Output As #stream'))
 [void]$builder.AppendLine('  Print #stream, "PID|" & CStr(GetCurrentProcessId())')
 [void]$builder.AppendLine('  Print #stream, "VERSION|" & Application.Version')
 [void]$builder.AppendLine(('  Set lib = Application.Libraries('+(BasicString ([IO.Path]::GetFileName($base)))+')'))
 [void]$builder.AppendLine('  If lib Is Nothing Then Err.Raise 2003, , "Private staging library unavailable"')
 [void]$builder.AppendLine('  full = lib.FullName')
 [void]$builder.AppendLine('  If LCase(Right(full,4)) = ".pt9" Then full = Left(full,Len(full)-4)')
 [void]$builder.AppendLine(('  If LCase(full) <> LCase('+(BasicString $base)+') Then Err.Raise 2004, , "SDK opened a different library"'))
 [void]$builder.AppendLine('  DumpItems lib, "BEFORE", stream')
 if($job.phase -eq 'convert') {
  $groups=@('parts','decals','symbols','drawings')
  for($kind=0;$kind -lt 4;$kind++) {
   # The first application checks every name before any import. A later Logic
   # pass must permit the PCB decals imported by this same private batch.
   if(-not $first -and $kind -eq 1) {continue}
   foreach($name in @($library.expected.($groups[$kind]))) {
    if([string]::IsNullOrEmpty([string]$name)) {continue}
    [void]$builder.AppendLine(('  Set items = lib.GetLibraryItems('+[string]$kind+', '+(BasicString ([string]$name))+')'))
    [void]$builder.AppendLine('  If items.Count <> 0 Then Err.Raise 2005, , "Same-name item already exists; replacement refused"')
   }
  }
  $inputs=@()
  if($role -eq 'layout' -and $library.pcb) {$inputs+=@([string]$library.pcb)}
  if($role -eq 'logic' -and $library.schematic) {$inputs+=@([string]$library.schematic)}
  if($role -eq [string]$library.partTypeEngine -and $library.partTypes) {$inputs+=@([string]$library.partTypes)}
  foreach($asciiInput in $inputs) {
   # The conflict policy is enforced by exact-name preflight and by Go's
   # before/after comparison, never by assuming the SDK skipped a duplicate.
   [void]$builder.AppendLine(('  Set items = lib.ImportLibraryItems2('+(BasicString $asciiInput)+', 2)'))
  }
 }
 [void]$builder.AppendLine('  DumpItems lib, "AFTER", stream')
 [void]$builder.AppendLine(('  Print #stream, '+(BasicString ('DONE|'+$job.nonce))))
 [void]$builder.AppendLine('  Close #stream')
 [void]$builder.AppendLine('  Application.Visible = True')
 [void]$builder.AppendLine('  Exit Sub')
 [void]$builder.AppendLine('Failed:')
 [void]$builder.AppendLine('  On Error Resume Next')
 [void]$builder.AppendLine('  If stream = 0 Then stream = FreeFile')
 [void]$builder.AppendLine(('  Open '+(BasicString $record)+' For Append As #stream'))
 [void]$builder.AppendLine('  Print #stream, "ERROR|" & Err.Description')
 [void]$builder.AppendLine('  Close #stream')
 [void]$builder.AppendLine('End Sub')
 return $builder.ToString()
}
