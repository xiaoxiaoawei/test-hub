# A Basic DONE marker is not a clean CAD exit. Call this before accepting any
# evidence, and only with the Process object returned by this batch's Start.
function Confirm-StudioPADSPID([string]$Reported,[int]$Expected) {
 $reportedPID=0
 if(-not [int]::TryParse($Reported,[ref]$reportedPID) -or $reportedPID -le 0 -or $reportedPID -ne $Expected) {throw 'PADS evidence was not written by the captured owned process.'}
}
function Complete-StudioPADSProcess($Process,[int]$TimeoutMilliseconds=5000) {
 if($null -eq $Process) {throw 'Owned PADS process is missing.'}
 if($TimeoutMilliseconds -lt 100 -or $TimeoutMilliseconds -gt 10000) {throw 'Invalid owned-process close timeout.'}
 if(-not $Process.HasExited) {
  $requested=$Process.CloseMainWindow()
  # A natural exit can race the close request. It is acceptable only after the
  # same process has actually exited and the exit code below has been checked.
  if(-not $requested -and -not $Process.HasExited) {throw 'Owned PADS has no closable main window; clean exit was not confirmed.'}
  if(-not $Process.WaitForExit($TimeoutMilliseconds)) {throw 'Owned PADS did not exit cleanly before the deadline; native output was rejected.'}
 }
 if($Process.ExitCode -ne 0) {throw ('Owned PADS exited with code '+$Process.ExitCode+'; native output was rejected.')}
}
