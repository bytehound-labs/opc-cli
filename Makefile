.PHONY: all debug release build test verify package package-win7 logs commit release-merge clean

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

# Creates a modern (Win10+) deployment zip via PowerShell single source of truth
package:
	pwsh -File scripts/package.ps1 -Task package

# Creates a Win7 / Server 2008 R2 legacy deployment zip
package-win7:
	pwsh -File scripts/package-win7.ps1

# Inspects application log file
logs:
	pwsh -File scripts/check-logs.ps1

# Runs quality gate, stages, commits, and pushes to remote
commit:
	pwsh -File scripts/commit.ps1 -Message "$(MSG)"

# Clean release merge from dev to main
release-merge:
	pwsh -File scripts/Merge-ToMain.ps1

clean:
	cargo clean
	rm -rf dist
