# OPC Core Components Redistributable

Place the `OPCCoreComponentsRedistributable_x64.msi` file in this directory.

## Download Source
- OPC Foundation: https://opcfoundation.org/developer-tools/samples-and-tools-classic
- Or obtain from your OPC server vendor's installation media.

## Usage
The `scripts/package-win7.ps1` script will automatically include
any `.msi` files from this directory in the legacy release bundle
under `redist/`.
