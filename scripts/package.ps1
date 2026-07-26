param (
    [string]$Task = "debug"
)

switch ($Task) {
    "debug"   { cargo build }
    "release" { cargo build --release }
    "build"   { cargo build --release }
    "test"    { cargo test }
    "package" {
        cargo build --release
        $distDir = "dist/opc-cli-x64"
        if (Test-Path $distDir) { Remove-Item -Recurse -Force $distDir }
        New-Item -ItemType Directory -Force -Path $distDir | Out-Null
        Copy-Item target/release/opc-cli.exe "$distDir/"
        Copy-Item -ErrorAction SilentlyContinue target/release/opc-cli.pdb "$distDir/"
        Copy-Item README.md "$distDir/"
        $zipPath = "dist/opc-cli-x64.zip"
        if (Test-Path $zipPath) { Remove-Item $zipPath }
        Compress-Archive -Path "$distDir/*" -DestinationPath $zipPath -Force
        Write-Host "Package created: $zipPath" -ForegroundColor Green
    }
    "package-win7" {
        & "$PSScriptRoot/package-win7.ps1"
    }
    Default { Write-Error "Unknown task: $Task" }
}
