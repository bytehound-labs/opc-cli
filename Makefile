.PHONY: all debug release build test verify package package-win7 clean

all: debug

debug:
	cargo build

release:
	cargo build --release

build: release

# Quick test — for full quality gate use 'make verify'
test:
	cargo test

verify:
	pwsh -File scripts/verify.ps1

# Creates a modern (Win10+) deployment zip
package: release
	mkdir -p dist/opc-cli-x64
	cp target/release/opc-cli.exe dist/opc-cli-x64/
	cp target/release/opc-cli.pdb dist/opc-cli-x64/ || true
	cp README.md dist/opc-cli-x64/
	tar -a -c -f dist/opc-cli-x64.zip dist/opc-cli-x64/*

# Creates a Win7 / Server 2008 R2 legacy deployment zip
package-win7:
	pwsh -File scripts/package-win7.ps1

clean:
	cargo clean
	rm -rf dist
