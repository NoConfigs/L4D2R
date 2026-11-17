param([string]$OutputPath, [switch]$Test)
$ErrorActionPreference = 'Stop'
$launcherRoot = Split-Path -Parent $PSScriptRoot
if (-not $OutputPath) { $OutputPath = Join-Path $launcherRoot 'L4D2R Launcher.exe' }
$launcherCompiler = Join-Path $env:WINDIR 'Microsoft.NET/Framework64/v4.0.30319/csc.exe'
if (-not (Test-Path -LiteralPath $launcherCompiler)) {
    $launcherCompiler = Join-Path $env:WINDIR 'Microsoft.NET/Framework/v4.0.30319/csc.exe'
}
if (-not (Test-Path -LiteralPath $launcherCompiler)) { throw 'The launcher compiler requires the Windows .NET Framework 4.x development compiler.' }
$launcherSources = Get-ChildItem -LiteralPath (Join-Path $launcherRoot 'launcher') -Filter '*.cs' -File | Select-Object -ExpandProperty FullName
& $launcherCompiler /nologo /target:winexe /platform:anycpu /optimize+ "/out:$OutputPath" "/win32manifest:$(Join-Path $launcherRoot 'launcher/app.manifest')" "/win32icon:$(Join-Path $launcherRoot 'launcher/app.ico')" "/resource:$(Join-Path $launcherRoot 'launcher/assets/logo.png'),L4D2R.Launcher.logo.png" /reference:System.Windows.Forms.dll /reference:System.Drawing.dll /reference:System.Core.dll $launcherSources
if ($LASTEXITCODE -ne 0) { throw 'Launcher compilation failed.' }
Write-Host "Launcher created: $OutputPath"
if ($Test) {
    $launcherTestProcess = Start-Process -FilePath $OutputPath -ArgumentList '--self-test' -PassThru -Wait -WindowStyle Hidden
    if ($launcherTestProcess.ExitCode -ne 0) { throw 'Launcher tests failed. See local-research/launcher/self-test.txt.' }
    Write-Host 'Launcher self-tests passed.'
}
