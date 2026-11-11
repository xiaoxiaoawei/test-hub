param([string]$GoExe='go', [switch]$SkipAltium, [switch]$Package)
$ErrorActionPreference='Stop'
$previous=Get-Location
try {
  Set-Location -LiteralPath $PSScriptRoot
  python tools/release.py check
  if ($LASTEXITCODE -ne 0) { throw 'Version metadata check failed' }
  Push-Location frontend
  try {
    npm.cmd ci
    if ($LASTEXITCODE -ne 0) { throw 'Frontend dependency installation failed' }
    npm.cmd run test:ui
    if ($LASTEXITCODE -ne 0) { throw 'Frontend regression checks failed' }
    npm.cmd run build
    if ($LASTEXITCODE -ne 0) { throw 'Frontend build failed' }
  } finally { Pop-Location }
  $assetDir=Join-Path $PSScriptRoot 'uiassets'
  $releaseDir=Join-Path $PSScriptRoot 'build\PackageStudio'
  foreach ($target in @($assetDir, $releaseDir)) {
    $resolved=[IO.Path]::GetFullPath($target)
    if (-not $resolved.StartsWith([IO.Path]::GetFullPath($PSScriptRoot)+[IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'Build target is outside repository' }
    if (Test-Path -LiteralPath $resolved) { Remove-Item -LiteralPath $resolved -Recurse -Force }
    New-Item -ItemType Directory -Path $resolved -Force | Out-Null
  }
  Get-ChildItem -LiteralPath 'frontend\dist' | Copy-Item -Destination 'uiassets' -Recurse -Force
  & $GoExe run github.com/tc-hib/go-winres@v0.3.3 make --arch amd64
  if ($LASTEXITCODE -ne 0) { throw 'PE resource generation failed' }
  & $GoExe test ./...
  if ($LASTEXITCODE -ne 0) { throw 'Go tests failed' }
  & $GoExe build -tags 'desktop,production' -trimpath -ldflags '-s -w -H windowsgui' -o (Join-Path $releaseDir 'PackageStudio.exe') .
  if ($LASTEXITCODE -ne 0) { throw 'Desktop build failed' }
  $version=(Get-Content -LiteralPath VERSION -Raw).Trim()
  $pe=(Get-Item -LiteralPath (Join-Path $releaseDir 'PackageStudio.exe')).VersionInfo
  if ($pe.FileVersion -ne "$version.0" -or $pe.ProductVersion -ne $version) { throw 'Embedded Windows version does not match VERSION' }
  if (-not $SkipAltium -and (Get-Command dotnet -ErrorAction SilentlyContinue)) {
    dotnet restore helpers/altium/PackageStudio.Altium.csproj -r win-x64 --locked-mode
    if ($LASTEXITCODE -ne 0) { throw 'Altium dependency lock verification failed' }
    dotnet publish helpers/altium/PackageStudio.Altium.csproj --no-restore -c Release -r win-x64 --self-contained false -o (Join-Path $releaseDir 'adapters\altium') -p:DebugType=None -p:DebugSymbols=false
    if ($LASTEXITCODE -ne 0) { throw 'Altium adapter build failed' }
  }
  if ($Package) {
    python tools/notices.py $GoExe
    if ($LASTEXITCODE -ne 0) { throw 'Dependency notice collection failed' }
    python -m unittest discover -s tools -p 'test_*.py'
    if ($LASTEXITCODE -ne 0) { throw 'Release gate tests failed' }
    python tools/release.py package
    if ($LASTEXITCODE -ne 0) { throw 'Release packaging failed' }
  }
  Write-Output ('Build completed: '+(Join-Path $releaseDir 'PackageStudio.exe'))
} finally { Set-Location -LiteralPath $previous }
