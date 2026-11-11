param([Parameter(Mandatory=$true)][string]$Request)
$ErrorActionPreference='Stop'
$ProgressPreference='SilentlyContinue'
Set-StrictMode -Version Latest

# This adapter is intentionally fail-closed. Classic 9.4 documents /sXXX and
# Library.ImportLibraryItems2. VX/Pro and unknown private-INI behavior are not
# silently accepted. A private config nonce is checked inside the CAD process
# before it reads a library. No original destination is present in this job.
if([IntPtr]::Size -ne 4) {throw 'PADS adapter requires 32-bit Windows PowerShell.'}
$job=Get-Content -LiteralPath $Request -Raw -Encoding UTF8 | ConvertFrom-Json
if($job.protocol -ne 1 -or $job.nonce -notmatch '^[a-f0-9]{48}$' -or $job.phase -notin @('convert','verify')) {throw 'Invalid PADS job protocol.'}
function RequireClosedCAD {
 if(@(Get-Process -Name powerpcb,powerlogic -ErrorAction SilentlyContinue).Count -ne 0) {throw 'STUDIO_PADS_BUSY: Save and close all PADS Logic/Layout instances before native conversion. Existing processes were not terminated.'}
}
RequireClosedCAD
$batch=[IO.Path]::GetFullPath([string]$job.root)
if([IO.Path]::GetFileName($batch) -notlike '.packagestudio-pads-native-*') {throw 'Invalid PADS staging directory.'}
function Inside([string]$path) {
 $full=[IO.Path]::GetFullPath($path)
 if(-not $full.StartsWith($batch+[IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase)) {throw 'PADS helper path is outside its private batch.'}
 for($cursor=$full;$cursor;$cursor=[IO.Path]::GetDirectoryName($cursor)) {
  if(Test-Path -LiteralPath $cursor) {if((Get-Item -LiteralPath $cursor -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) {throw 'Redirected PADS staging path.'}}
  if($cursor -eq $batch) {break}
 }
 return $full
}
[void](Inside $Request);$response=Inside ([string]$job.response)
if(Test-Path -LiteralPath $response) {throw 'PADS response already exists.'}
$extensions=@('.pt9','.pd9','.ld9','.ln9')
foreach($library in $job.libraries) {
 [void](Inside ([string]$library.base))
 foreach($field in @('pcb','schematic','partTypes')) {if($library.$field) {[void](Inside ([string]$library.$field))}}
}
foreach($property in $job.sdk.hashes.psobject.Properties) {
 if((Get-FileHash -LiteralPath $property.Name -Algorithm SHA256).Hash -ine [string]$property.Value) {throw 'Selected PADS SDK changed after inspection.'}
}
Add-Type -Path (Join-Path $PSScriptRoot 'Factory.cs')
[StudioPadsFactory]::BindLifetime()
$env:SDD_HOME=[string]$job.sdk.root
$env:PATH=(Join-Path ([string]$job.sdk.root) 'Programs')+';'+(Join-Path ([string]$job.sdk.root) 'common\win32\lib')+';'+$env:PATH
function TextFile([string]$path,[string]$text) {
 $encoding=[Text.Encoding]::Default
 if($encoding.GetString($encoding.GetBytes($text)) -cne $text) {throw 'PADS 9.4 paths cannot be represented in the system code page; choose an ASCII output path.'}
 [IO.File]::WriteAllText((Inside $path),$text,$encoding)
}
. (Join-Path $PSScriptRoot 'basic.ps1')
function EmptyItems {return @{parts=(New-Object Collections.ArrayList);decals=(New-Object Collections.ArrayList);symbols=(New-Object Collections.ArrayList);drawings=(New-Object Collections.ArrayList)}}
function ReadInventory([string]$path) {
 $before=EmptyItems;$after=EmptyItems;$done=$false;$version=''
 $lines=[IO.File]::ReadAllLines((Inside $path),[Text.Encoding]::Default)
 if($lines.Length -gt 800005) {throw 'PADS inventory is too large.'}
 foreach($line in $lines) {
  $row=$line.Split('|')
  if($row[0] -eq 'ERROR') {throw ('PADS SDK error: '+($row[1..($row.Length-1)] -join '|'))}
  if($row[0] -eq 'VERSION' -and $row.Length -eq 2) {$version=$row[1];continue}
  if($line -eq ('DONE|'+$job.nonce)) {$done=$true;continue}
  if($row[0] -notin @('BEFORE','AFTER')) {continue}
  if($row.Length -ne 3 -or $row[1] -notin @('0','1','2','3') -or -not $row[2]) {throw 'Malformed PADS inventory.'}
  $name=@('parts','decals','symbols','drawings')[[int]$row[1]]
  if($row[0] -eq 'BEFORE') {[void]$before[$name].Add($row[2])} else {[void]$after[$name].Add($row[2])}
 }
 if(-not $done -or $version -notmatch '(^|[^0-9])9\.4([^0-9]|$)') {throw 'PADS SDK did not finish a supported 9.4 readback.'}
 return @{before=$before;after=$after}
}

function RunCAD($library,[string]$role,[bool]$first) {
 $selected=if($role -eq 'logic') {[string]$job.sdk.logic} else {[string]$job.sdk.layout}
 if(-not $selected) {throw 'Required PADS application was not selected.'}
 $identity=[Diagnostics.FileVersionInfo]::GetVersionInfo($selected)
 if(($identity.ProductName+' '+$identity.FileDescription) -notmatch '(?i)PADS|PowerPCB|PowerLogic' -or
    ($identity.ProductVersion+' '+$identity.FileVersion) -notmatch '(^|[^0-9])9\.4([^0-9]|$)') {throw 'Only genuine PADS Classic 9.4 metadata is accepted by this pending-validation adapter.'}
 $stem=if($role -eq 'logic') {'powerlogic'} else {'powerpcb'}
 $runRoot=Inside (Join-Path $batch ($job.phase+'-'+$library.id+'-'+$role))
 $programs=Inside (Join-Path $runRoot 'Programs');[IO.Directory]::CreateDirectory($programs) | Out-Null
 $privateExe=Inside (Join-Path $programs ($stem+'.exe'))
 [IO.File]::Copy($selected,$privateExe,$false)
 if((Get-FileHash -LiteralPath $selected).Hash -ne (Get-FileHash -LiteralPath $privateExe).Hash) {throw 'Private SDK executable copy mismatch.'}
 $common=Inside (Join-Path $runRoot 'common');[IO.Directory]::CreateDirectory($common) | Out-Null
 $commonConfig=Join-Path ([string]$job.sdk.root) 'common\config.ini'
 if(Test-Path -LiteralPath $commonConfig) {[IO.File]::Copy($commonConfig,(Join-Path $common 'config.ini'),$false)}
 $extension=if($role -eq 'logic') {'.sch'} else {'.pcb'}
 $template=Inside (Join-Path $programs ('default'+$extension))
 [IO.File]::Copy((Join-Path ([string]$job.sdk.root) ('Settings\default'+$extension)),$template,$false)
 $originalIni=Join-Path ([IO.Path]::GetDirectoryName($selected)) ($stem+'.ini')
 if(-not (Test-Path -LiteralPath $originalIni)) {throw 'Selected PADS installation has no adjacent INI; private startup is not adapted.'}
 $ini=[IO.File]::ReadAllText($originalIni,[Text.Encoding]::Default)
 # Only these independently generated private files are changed. The CAD-side
 # guard below rejects any version that ignores them, before any library call.
 $ini=[regex]::Replace($ini,'(?ims)^\[library_list\][^\[]*','')
 $base=Inside ([string]$library.base)
 $ini+="`r`n[Library_List]`r`nSynchronize=0`r`nLib1=`""+[IO.Path]::GetFileName($base)+"`" `""+[IO.Path]::GetDirectoryName($base)+"\`" YYY`r`n[PackageStudioNative]`r`nNonce="+$job.nonce+"`r`n"
 TextFile (Join-Path $programs ($stem+'.ini')) $ini
 $record=Inside (Join-Path $programs 'inventory.txt');$script=Inside (Join-Path $programs 'studio.bas')
 TextFile $script (New-StudioPADSBasic $job $library $role $first $privateExe $record $base)
 $start=New-Object Diagnostics.ProcessStartInfo
 $start.FileName=$privateExe;$start.WorkingDirectory=$programs;$start.UseShellExecute=$false;$start.CreateNoWindow=$true;$start.WindowStyle=[Diagnostics.ProcessWindowStyle]::Hidden
 # Do not send a document filename before the in-process guards: a version may
 # forward it to an existing session. Library automation needs no active design.
 $start.Arguments='"/s'+$script+'"'
 $start.EnvironmentVariables['SDD_HOME']=[string]$job.sdk.root
 $start.EnvironmentVariables['PATH']=(Join-Path ([string]$job.sdk.root) 'Programs')+';'+(Join-Path ([string]$job.sdk.root) 'common\win32\lib')+';'+$env:PATH
 $process=$null
 try {
  RequireClosedCAD
  $process=[Diagnostics.Process]::Start($start)
  $deadline=[DateTime]::UtcNow.AddSeconds(60)
  while([DateTime]::UtcNow -lt $deadline) {
   if(Test-Path -LiteralPath $record) {try {$text=[IO.File]::ReadAllText($record,[Text.Encoding]::Default);if($text.Contains('DONE|'+$job.nonce) -or $text.Contains('ERROR|')) {break}} catch {}}
   if($process.HasExited) {break};Start-Sleep -Milliseconds 100
  }
  if(-not (Test-Path -LiteralPath $record)) {throw 'Private PADS startup produced no guarded inventory; dependencies/INI/CLI are not compatible.'}
  $inventory=ReadInventory $record
  return $inventory
 } finally {
  # Only the process handle returned for our private executable is affected.
  # No taskkill by image name, ROT attachment or current-user CAD is used.
  if($null -ne $process) {try {if(-not $process.HasExited) {[void]$process.CloseMainWindow();if(-not $process.WaitForExit(1500)) {$process.Kill();$process.WaitForExit()}}} finally {$process.Dispose()}}
 }
}

$result=@{protocol=1;nonce=[string]$job.nonce;phase=[string]$job.phase;root=$batch;engine='PADS';nativeGenerated=$true;runtimeVerified=($job.phase -eq 'verify');libraries=@();warnings=@('PADS Classic 9.4 private startup and SDK compatibility require real EDA acceptance; global configuration fallback is disabled.')}
if($job.phase -eq 'convert') {
 $factory=[StudioPadsFactory]::Open([string]$job.sdk.factory)
 try {foreach($library in $job.libraries) {if(-not $library.existing) {foreach($extension in $extensions) {$path=Inside ($library.base+$extension);if(Test-Path -LiteralPath $path) {throw 'New native library already exists.'};$factory.Pads_CreateLibraryFile($path);if(-not (Test-Path -LiteralPath $path) -or (Get-Item -LiteralPath $path).Length -eq 0) {throw 'Native factory produced an empty library.'}}}}}
 finally {if($null -ne $factory) {[void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($factory)}}
}
foreach($library in $job.libraries) {
 foreach($extension in $extensions) {if(-not (Test-Path -LiteralPath ($library.base+$extension))) {throw 'Incomplete native four-file library.'}}
 $before=$null;$after=$null
 if($job.phase -eq 'verify') {
  $role=if($library.schematic) {'logic'} else {'layout'}
  $read=RunCAD $library $role $true;$before=$read.before;$after=$read.after
 } else {
  if($library.pcb -or $library.partTypeEngine -eq 'layout') {$read=RunCAD $library 'layout' $true;$before=$read.before;$after=$read.after}
  if($library.schematic) {$read=RunCAD $library 'logic' ($null -eq $before);if($null -eq $before) {$before=$read.before};$after=$read.after}
 }
 if($null -eq $before -or $null -eq $after) {throw 'No selected SDK completed the library operation.'}
 $result.libraries+=@(@{id=[string]$library.id;base=[string]$library.base;before=$before;after=$after;reopened=($job.phase -eq 'verify')})
}
[IO.File]::WriteAllText($response,($result | ConvertTo-Json -Depth 15),(New-Object Text.UTF8Encoding($false)))
