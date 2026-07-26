<#
.SYNOPSIS
    Universal Task & Packaging Dispatcher for opc-cli.
.DESCRIPTION
    Provides a single PowerShell entry point for all workspace build,
    verification, packaging, log inspection, and release operations.
.PARAMETER Task
    The operation to execute: debug, release, build, test, verify, package, package-win7, logs, commit, release-merge.
.PARAMETER Message
    Optional message parameter passed to commit or release-merge tasks.
#>

[CmdletBinding()]
param (
    [ValidateSet("debug", "release", "build", "test", "verify", "package", "package-win7", "logs", "commit", "release-merge")]
    [string]$Task = "debug",
    [string]$Message
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$RepoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $RepoRoot

switch ($Task) {
    "debug" {
        cargo build
    }
    "release" {
        cargo build --release
    }
    "build" {
        cargo build --release
    }
    "test" {
        cargo test --workspace
    }
    "verify" {
        & "$PSScriptRoot/verify.ps1"
    }
    "package" {
        cargo build --release --bin opc-cli
        $distDir = "dist/opc-cli-x64"
        if (Test-Path $distDir) { Remove-Item -Recurse -Force $distDir }
        New-Item -ItemType Directory -Force -Path $distDir | Out-Null
        Copy-Item target/release/opc-cli.exe "$distDir/"
        Copy-Item -ErrorAction SilentlyContinue target/release/opc-cli.pdb "$distDir/"
        Copy-Item README.md "$distDir/"
        $zipPath = "dist/opc-cli-x64.zip"
        if (Test-Path $zipPath) { Remove-Item $zipPath }
        Compress-Archive -Path "$distDir/*" -DestinationPath $zipPath -Force
        Write-Host "Modern package created: $zipPath" -ForegroundColor Green
    }
    "package-win7" {
        & "$PSScriptRoot/package-win7.ps1"
    }
    "logs" {
        & "$PSScriptRoot/check-logs.ps1"
    }
    "commit" {
        if ($Message) {
            & "$PSScriptRoot/commit.ps1" -Message $Message
        } else {
            & "$PSScriptRoot/commit.ps1"
        }
    }
    "release-merge" {
        if ($Message) {
            & "$PSScriptRoot/Merge-ToMain.ps1" -Message $Message
        } else {
            & "$PSScriptRoot/Merge-ToMain.ps1"
        }
    }
}
