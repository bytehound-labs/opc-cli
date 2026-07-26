<#
.SYNOPSIS
    Automated Win7 / Windows Server 2008 R2 Legacy Release Packaging Script.
.DESCRIPTION
    Builds opc-cli with static CRT linking, compiles polyfill DLLs, PE-patches
    missing NT 6.1 imports (GetSystemTimePreciseAsFileTime -> GetSystemTimeAsFileTime),
    and packages a self-contained release bundle in dist/opc-cli-win7-x64.
#>

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$RepoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $RepoRoot

Write-Host "========================================================" -ForegroundColor Cyan
Write-Host " Building Win7 / Server 2008 R2 Legacy Release Bundle" -ForegroundColor Cyan
Write-Host "========================================================" -ForegroundColor Cyan

# Prerequisite Checks
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Error "cargo command not found. Please install Rust toolchain."
}

# Step 1: Build opc-cli with static CRT linking
Write-Host "`n[1/6] Compiling opc-cli (Release + static CRT)..." -ForegroundColor Yellow
$env:RUSTFLAGS = "-C target-feature=+crt-static"
try {
    cargo build --release --bin opc-cli
} finally {
    $env:RUSTFLAGS = ""
}

$ExePath = "target/release/opc-cli.exe"
if (-not (Test-Path $ExePath)) {
    Write-Error "Failed to locate compiled binary at $ExePath"
}

# Step 2: Build Polyfill Crates
Write-Host "`n[2/6] Compiling Polyfill Crates (standalone)..." -ForegroundColor Yellow
$polyfills = @(
    @{ Manifest = "compat/synch-polyfill/Cargo.toml";
       Src = "compat/synch-polyfill/target/release/api_ms_win_core_synch_l1_2_0.dll";
       Dst = "api-ms-win-core-synch-l1-2-0.dll" },
    @{ Manifest = "compat/winrt-error-polyfill/Cargo.toml";
       Src = "compat/winrt-error-polyfill/target/release/api_ms_win_core_winrt_error_l1_1_0.dll";
       Dst = "api-ms-win-core-winrt-error-l1-1-0.dll" },
    @{ Manifest = "compat/bcrypt-polyfill/Cargo.toml";
       Src = "compat/bcrypt-polyfill/target/release/bcryptprimitives.dll";
       Dst = "bcryptprimitives.dll" }
)

foreach ($p in $polyfills) {
    Write-Host "  -> Building $($p.Manifest)..." -ForegroundColor DarkGray
    cargo build --manifest-path $p.Manifest --release
    if (-not (Test-Path $p.Src)) {
        Write-Error "Failed to build polyfill DLL: $($p.Src)"
    }
    $dllSize = (Get-Item $p.Src).Length
    if ($dllSize -lt 4096) {
        Write-Error "Polyfill DLL '$($p.Src)' is suspiciously small ($dllSize bytes). Build may have failed silently."
    }
    Write-Host "    Size: $([math]::Round($dllSize / 1024, 1)) KB" -ForegroundColor DarkGray
}

# Step 3: PE Import Table Binary Patching
Write-Host "`n[3/6] PE Patching GetSystemTimePreciseAsFileTime -> GetSystemTimeAsFileTime..." -ForegroundColor Yellow
$bytes = [System.IO.File]::ReadAllBytes($ExePath)
$search = [System.Text.Encoding]::ASCII.GetBytes("GetSystemTimePreciseAsFileTime")
$replace = [System.Text.Encoding]::ASCII.GetBytes("GetSystemTimeAsFileTime")

# Prepare replacement byte array padded with NUL bytes to match 30-byte length
$padded = New-Object byte[] $search.Length
[Array]::Copy($replace, $padded, $replace.Length)

$patchedCount = 0
for ($i = 0; $i -le ($bytes.Length - $search.Length); $i++) {
    $match = $true
    for ($j = 0; $j -lt $search.Length; $j++) {
        if ($bytes[$i + $j] -ne $search[$j]) {
            $match = $false
            break
        }
    }
    if ($match) {
        [Array]::Copy($padded, 0, $bytes, $i, $padded.Length)
        $patchedCount++
        $i += $search.Length - 1
    }
}

if ($patchedCount -eq 0) {
    Write-Host "  [WARN] String 'GetSystemTimePreciseAsFileTime' not found in binary (already patched or absent)." -ForegroundColor Yellow
} else {
    Write-Host "  [OK] Patched $patchedCount import table occurrence(s)." -ForegroundColor Green
    [System.IO.File]::WriteAllBytes($ExePath, $bytes)
}

# Post-patch validation: ensure no stale import remains
$patchedBytes = [System.IO.File]::ReadAllBytes($ExePath)
$staleFound = $false
for ($i = 0; $i -le ($patchedBytes.Length - $search.Length); $i++) {
    $match = $true
    for ($j = 0; $j -lt $search.Length; $j++) {
        if ($patchedBytes[$i + $j] -ne $search[$j]) {
            $match = $false
            break
        }
    }
    if ($match) {
        $staleFound = $true
        break
    }
}
if ($staleFound) {
    Write-Error "PE patch validation FAILED: 'GetSystemTimePreciseAsFileTime' still present in binary after patching."
}
Write-Host "  [OK] Post-patch validation passed — no stale imports remain." -ForegroundColor Green

# Step 4: Assemble Dist Directory
Write-Host "`n[4/6] Assembling dist/opc-cli-win7-x64 directory..." -ForegroundColor Yellow
$DistDir = "dist/opc-cli-win7-x64"
if (Test-Path $DistDir) {
    Remove-Item -Recurse -Force $DistDir
}
New-Item -ItemType Directory -Force -Path $DistDir | Out-Null

Copy-Item $ExePath "$DistDir/"
Copy-Item -ErrorAction SilentlyContinue "target/release/opc-cli.pdb" "$DistDir/"
Copy-Item "README.md" "$DistDir/"

foreach ($p in $polyfills) {
    Copy-Item $p.Src "$DistDir/$($p.Dst)"
    Write-Host "  [+] Included $($p.Dst)" -ForegroundColor DarkGray
}

# Step 5: Copy Redistributables if present
Write-Host "`n[5/6] Checking vendor/redist for OPC Core Components..." -ForegroundColor Yellow
$RedistSrc = "vendor/redist"
if (Test-Path $RedistSrc) {
    $msiFiles = @(Get-ChildItem -Path $RedistSrc -Filter "*.msi")
    if ($msiFiles.Count -gt 0) {
        $RedistDst = "$DistDir/redist"
        New-Item -ItemType Directory -Force -Path $RedistDst | Out-Null
        foreach ($msi in $msiFiles) {
            Copy-Item $msi.FullName "$RedistDst/"
            Write-Host "  [+] Included redistributable: $($msi.Name)" -ForegroundColor Green
        }
    } else {
        Write-Host "  [INFO] No .msi redistributables found in vendor/redist/ (skipping redist folder)." -ForegroundColor DarkGray
    }
}

# Step 6: Create Zip Archive
Write-Host "`n[6/6] Compressing release bundle..." -ForegroundColor Yellow
$ZipPath = "dist/opc-cli-win7-x64.zip"
if (Test-Path $ZipPath) {
    Remove-Item $ZipPath
}
Compress-Archive -Path "$DistDir/*" -DestinationPath $ZipPath -Force

Write-Host "`n========================================================" -ForegroundColor Green
Write-Host " Win7 / Server 2008 R2 Release Packaging Complete! ✅" -ForegroundColor Green
Write-Host " Package: $ZipPath" -ForegroundColor Green
Write-Host " Directory: $DistDir/" -ForegroundColor Green
Write-Host "========================================================" -ForegroundColor Green
