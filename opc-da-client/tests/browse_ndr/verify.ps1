$ErrorActionPreference = 'Stop'

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..')).Path
$buildDirectory = Join-Path $repoRoot 'target\browse-ndr-probe'
New-Item -ItemType Directory -Force -Path $buildDirectory | Out-Null

$programFilesX86 = [Environment]::GetFolderPath('ProgramFilesX86')
$vswhere = Join-Path $programFilesX86 'Microsoft Visual Studio\Installer\vswhere.exe'
if (-not (Test-Path $vswhere)) {
    throw "Visual Studio locator not found: $vswhere"
}

$visualStudio = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($visualStudio)) {
    throw 'Visual Studio C++ build tools were not found'
}

$developerCommand = Join-Path $visualStudio 'Common7\Tools\VsDevCmd.bat'
if (-not (Test-Path $developerCommand)) {
    throw "Visual Studio developer command not found: $developerCommand"
}

$windowsKitsBin = Join-Path $programFilesX86 'Windows Kits\10\bin'
$midl = Get-ChildItem -Path $windowsKitsBin -Directory |
    Sort-Object { [version]$_.Name } -Descending |
    ForEach-Object { Join-Path $_.FullName 'x64\midl.exe' } |
    Where-Object { Test-Path $_ } |
    Select-Object -First 1
if (-not $midl) {
    throw "Windows SDK MIDL compiler not found under $windowsKitsBin"
}

$idl = Join-Path $PSScriptRoot 'browse_property_array_probe.idl'
$serverSource = Join-Path $PSScriptRoot 'browse_property_array_probe_server.c'
$clientSource = Join-Path $PSScriptRoot 'browse_property_array_probe_client.c'
$buildScript = Join-Path $buildDirectory 'build-fixture.cmd'
$buildCommands = @(
    '@echo off',
    'setlocal',
    ('call "' + $developerCommand + '" -no_logo -host_arch=x64 -arch=x64'),
    'if errorlevel 1 exit /b %errorlevel%',
    ('cd /d "' + $buildDirectory + '"'),
    ('"' + $midl + '" /nologo /env win64 /robust /Oicf /h browse_property_array_probe.h /cstub browse_property_array_probe_c.c /sstub browse_property_array_probe_s.c /iid browse_property_array_probe_i.c "' + $idl + '"'),
    'if errorlevel 1 exit /b %errorlevel%',
    ('cl /nologo /W4 /TC /DWIN32_LEAN_AND_MEAN /I "' + $buildDirectory + '" /Fe:browse_property_array_probe_server.exe "' + $serverSource + '" browse_property_array_probe_s.c browse_property_array_probe_i.c /link Rpcrt4.lib'),
    'if errorlevel 1 exit /b %errorlevel%',
    ('cl /nologo /W4 /TC /DWIN32_LEAN_AND_MEAN /I "' + $buildDirectory + '" /Fe:browse_property_array_probe_client.exe "' + $clientSource + '" browse_property_array_probe_c.c browse_property_array_probe_i.c /link Rpcrt4.lib'),
    'exit /b %errorlevel%'
)
[IO.File]::WriteAllLines($buildScript, $buildCommands, [Text.Encoding]::ASCII)

Push-Location $buildDirectory
try {
    & $env:ComSpec /d /c "`"$buildScript`""
    if ($LASTEXITCODE -ne 0) {
        throw "MIDL RPC/NDR fixture build failed with exit code $LASTEXITCODE"
    }
}
finally {
    Pop-Location
}

$serverExecutable = Join-Path $buildDirectory 'browse_property_array_probe_server.exe'
$clientExecutable = Join-Path $buildDirectory 'browse_property_array_probe_client.exe'
$endpoint = 'opccli_browse_ndr_' + [Guid]::NewGuid().ToString('N')
$server = Start-Process -FilePath $serverExecutable -ArgumentList @('server', $endpoint) -PassThru -NoNewWindow

try {
    Start-Sleep -Milliseconds 100
    $client = Start-Process -FilePath $clientExecutable -ArgumentList @('client', $endpoint) -PassThru -NoNewWindow
    if (-not $client.WaitForExit(15000)) {
        Stop-Process -Id $client.Id -Force
        throw 'RPC/NDR fixture client exceeded the 15-second timeout'
    }
    if ($client.ExitCode -ne 0) {
        throw "RPC/NDR fixture client failed with exit code $($client.ExitCode)"
    }

    if (-not $server.WaitForExit(10000)) {
        Stop-Process -Id $server.Id -Force
        throw 'RPC/NDR fixture server did not exit after the shutdown request'
    }
    if ($server.ExitCode -ne 0) {
        throw "RPC/NDR fixture server failed with exit code $($server.ExitCode)"
    }

    Write-Output 'Out-of-process MIDL/NDR zero-property Browse probe passed'
}
finally {
    if (-not $server.HasExited) {
        Stop-Process -Id $server.Id -Force
    }
}
