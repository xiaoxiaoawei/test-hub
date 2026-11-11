param([Parameter(Mandatory=$true)][string]$Request)
$ErrorActionPreference='Stop'
$ProgressPreference='SilentlyContinue'
Set-StrictMode -Version Latest
if([IntPtr]::Size -ne 4) {throw 'Modern PADS candidate adapter requires x86 PowerShell.'}
$job=Get-Content -LiteralPath $Request -Raw -Encoding UTF8 | ConvertFrom-Json
if($job.protocol -ne 1 -or $job.nonce -notmatch '^[a-f0-9]{48}$' -or $job.phase -notin @('convert','verify')) {throw 'Invalid PADS model protocol.'}
$batch=[IO.Path]::GetFullPath([string]$job.root)
if([IO.Path]::GetFileName($batch) -notlike '.packagestudio-pads-models-*') {throw 'Invalid private batch.'}
function Inside([string]$path) {
 $full=[IO.Path]::GetFullPath($path)
 if(-not $full.StartsWith($batch+[IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase)) {throw 'Path is outside private model batch.'}
 for($cursor=$full;$cursor;$cursor=[IO.Path]::GetDirectoryName($cursor)) {
  if(Test-Path -LiteralPath $cursor) {if((Get-Item -LiteralPath $cursor -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) {throw 'Redirected model path.'}}
  if($cursor -eq $batch) {break}
 }
 return $full
}
function RequireClosedCAD {
 if(@(Get-Process -Name powerpcb,powerlogic -ErrorAction SilentlyContinue).Count -ne 0) {throw 'STUDIO_PADS_BUSY: Save and close all existing PADS sessions; none were terminated.'}
}
function WriteANSI([string]$path,[string]$text) {
 $encoding=[Text.Encoding]::Default
 if($encoding.GetString($encoding.GetBytes($text)) -cne $text) {throw 'Selected paths are not representable in the system code page.'}
 [IO.File]::WriteAllText((Inside $path),$text,$encoding)
}
[void](Inside $Request);$response=Inside ([string]$job.response)
if(Test-Path -LiteralPath $response) {throw 'Response already exists.'}
$reuse=Inside ([string]$job.reuse);[void](Inside ([string]$job.pcb));[void](Inside ([string]$job.partTypes))
RequireClosedCAD
$selected=[IO.Path]::GetFullPath([string]$job.sdk.executable)
$id=[Diagnostics.FileVersionInfo]::GetVersionInfo($selected)
if([IO.Path]::GetFileName($selected) -ine 'powerpcb.exe' -or ($id.ProductName+' '+$id.FileDescription) -notmatch '(?i)PADS|PowerPCB' -or $id.ProductVersion -notmatch '^2019\.2\.') {throw 'Only the candidate PADS Classic 2019.2 x86 profile is accepted.'}
foreach($hash in $job.sdk.hashes.psobject.Properties) {if((Get-FileHash -LiteralPath $hash.Name -Algorithm SHA256).Hash -ine [string]$hash.Value) {throw 'Selected SDK changed after inspection.'}}
Add-Type -Path (Join-Path $PSScriptRoot 'Factory.cs')
[StudioPadsFactory]::BindLifetime()
. (Join-Path $PSScriptRoot 'models_basic.ps1')
. (Join-Path $PSScriptRoot 'process.ps1')
$result=@{protocol=1;nonce=[string]$job.nonce;root=$batch;phase=[string]$job.phase;reuse=$reuse;version='';items=@()}
$index=0
foreach($item in @($job.items)) {
 if($item.entry -notmatch '^[A-Z0-9_.$-]{1,40}$' -or $item.decal -notmatch '^[A-Z0-9_.$-]{1,40}$' -or $item.modelName -notmatch '^[A-Za-z0-9_.$-]{1,80}$') {throw 'Invalid mapping identity.'}
 $step=Inside ([string]$item.step)
 if((Get-FileHash -LiteralPath $step -Algorithm SHA256).Hash -ine [string]$item.sha256) {throw 'Prepared STEP changed after staging.'}
 $run=Inside (Join-Path $batch ($job.phase+'-model-'+$index));$index++
 $programs=Inside (Join-Path $run 'Programs');[IO.Directory]::CreateDirectory($programs) | Out-Null
 $exe=Inside (Join-Path $programs 'powerpcb.exe');[IO.File]::Copy($selected,$exe,$false)
 if((Get-FileHash -LiteralPath $exe).Hash -ne (Get-FileHash -LiteralPath $selected).Hash) {throw 'Private executable copy mismatch.'}
 $template=Inside (Join-Path $run 'default.pcb');[IO.File]::Copy((Join-Path ([string]$job.sdk.root) 'Settings\default.pcb'),$template,$false)
 $common=Inside (Join-Path $run 'common');[IO.Directory]::CreateDirectory($common) | Out-Null
 $config=Join-Path ([string]$job.sdk.root) 'common\config.ini'
 if(Test-Path -LiteralPath $config) {[IO.File]::Copy($config,(Join-Path $common 'config.ini'),$false)}
 $base=Inside (Join-Path $run 'FixtureLibrary')
 $ini=[IO.File]::ReadAllText((Join-Path ([IO.Path]::GetDirectoryName($selected)) 'powerpcb.ini'),[Text.Encoding]::Default)
 $ini=[regex]::Replace($ini,'(?ims)^\[library_list\][^\[]*','')
 $ini=[regex]::Replace($ini,'(?ims)^\[directories\][^\[]*','')
 $ini+="`r`n[Library_List]`r`nSynchronize=0`r`nLib1=`"FixtureLibrary`" `""+$run+"\`" YYY`r`n[directories]`r`nReuseDir="+$reuse+"`r`n[PackageStudioModels]`r`nNonce="+$job.nonce+"`r`n"
 WriteANSI (Join-Path $programs 'powerpcb.ini') $ini
 $netlist=Inside (Join-Path $run 'fixture.asc')
 # PADS netlist's documented PART record: reference, part type. The selected
 # part type in our own private library determines the exact PCB decal.
 WriteANSI $netlist ("*PADS-PCB*`r`n*PART*`r`nPT1 "+$item.entry+"`r`n*NET*`r`n*END*`r`n")
 $board=Inside (Join-Path $run 'fixture.pcb');$record=Inside (Join-Path $run 'readback.txt');$basic=Inside (Join-Path $run 'models.bas')
 if(Test-Path -LiteralPath (Join-Path $run 'fixture_PCB')) {throw 'Fresh verification board already has a cache.'}
 WriteANSI $basic (New-StudioPADSModelBasic $job $item $exe $template $board $base $netlist $record)
 $start=New-Object Diagnostics.ProcessStartInfo
 $start.FileName=$exe;$start.WorkingDirectory=$programs;$start.UseShellExecute=$false
 # System3D may require its own native main window. Only our new PID is used.
 $start.WindowStyle=[Diagnostics.ProcessWindowStyle]::Normal
 $start.Arguments='"/s'+$basic+'"'
 $start.EnvironmentVariables['SDD_HOME']=[string]$job.sdk.root
 $start.EnvironmentVariables['PATH']=(Join-Path ([string]$job.sdk.root) 'Programs')+';'+(Join-Path ([string]$job.sdk.root) 'common\win32\lib')+';'+$env:PATH
 $process=$null
 try {
  RequireClosedCAD
  $process=[Diagnostics.Process]::Start($start)
  $deadline=[DateTime]::UtcNow.AddSeconds(90)
  $text=''
  while([DateTime]::UtcNow -lt $deadline) {
   if(Test-Path -LiteralPath $record) {try {$text=[IO.File]::ReadAllText($record,[Text.Encoding]::Default);if($text.Contains('ERROR|') -or $text.Contains('DONE|'+$job.nonce)) {break}} catch {}}
   if($process.HasExited) {break};Start-Sleep -Milliseconds 100
  }
  if($text.Contains('ERROR|')) {throw ('Native SDK refused operation: '+$text)}
  if(-not $text.Contains('DONE|'+$job.nonce)) {throw 'Private startup/API/authorization failed; no completed guarded readback.'}
  $values=@{}
  foreach($line in ($text -split "`r?`n")) {if(-not $line){continue};$row=$line.Split('|');if($values.ContainsKey($row[0])) {throw 'Duplicate SDK evidence field.'};$values[$row[0]]=$row}
  foreach($key in @('PID','VERSION','MODEL','LOCAL','BBOX','DONE')) {if(-not $values.ContainsKey($key)) {throw 'Incomplete model readback.'}}
  if($values.PID.Length -ne 2){throw 'Invalid model process PID record.'}
  Confirm-StudioPADSPID ([string]$values.PID[1]) $process.Id
  $version=[string]$values.VERSION[1]
  if($version -notmatch '^2019\.2\.') {throw 'System3D bridge version is not in the candidate profile.'}
  if($result.version -and $result.version -cne $version) {throw 'Bridge version changed during batch.'};$result.version=$version
  $model=[string]$values.MODEL[1]
  if($model -cne ([string]$item.modelName+'.step')) {throw 'Mapped model name differs from prepared STEP.'}
  $local=[bool]::Parse([string]$values.LOCAL[1])
  if($job.phase -eq 'verify' -and $local) {throw 'Fresh board used a local mapping instead of saved Reuse.'}
  $bbox=@();if($values.BBOX.Length -ne 7) {throw 'Invalid bounding-box report.'}
  for($n=1;$n -le 6;$n++) {$number=[double]::Parse([string]$values.BBOX[$n],[Globalization.CultureInfo]::CurrentCulture);if([double]::IsNaN($number) -or [double]::IsInfinity($number)) {throw 'Nonfinite native bounds.'};$bbox+=$number}
  $packageHash='';$loadedHash=''
  if($job.phase -eq 'verify') {
   $localFile=Inside (Join-Path $run 'fixture_PCB\3D\LocalModelMappings.dat')
   if((Test-Path -LiteralPath $localFile) -and (Get-Item -LiteralPath $localFile).Length -ne 0) {throw 'Fresh verification has a local mapping database.'}
   $package=Inside (Join-Path $reuse ('3DModels\PartDB\Packages\User\'+$model+'\SAT\'+$model+'_1.xtd'))
   $loaded=Inside (Join-Path $run ('fixture_PCB\3D\Input\'+$model+'.xtd'))
   $packageHash=(Get-FileHash -LiteralPath $package -Algorithm SHA256).Hash.ToLowerInvariant()
   $loadedHash=(Get-FileHash -LiteralPath $loaded -Algorithm SHA256).Hash.ToLowerInvariant()
   if($packageHash -cne $loadedHash) {throw 'Freshly loaded geometry differs from persisted Reuse package.'}
  }
  Complete-StudioPADSProcess $process
  $result.items+=@(@{partId=[string]$item.partId;entry=[string]$item.entry;decal=[string]$item.decal;modelName=$model;mapped=$true;local=$local;bboxMm=$bbox;packageSha256=$packageHash;loadedSha256=$loadedHash;freshDocument=$true})
 } finally {
  # This is failure cleanup only. A forced kill can never produce success.
  if($null -ne $process) {try {if(-not $process.HasExited) {$process.Kill();if(-not $process.WaitForExit(5000)) {throw 'Owned PADS cleanup did not finish.'}}} finally {$process.Dispose()}}
 }
}
[IO.File]::WriteAllText($response,($result | ConvertTo-Json -Depth 20),(New-Object Text.UTF8Encoding($false)))
