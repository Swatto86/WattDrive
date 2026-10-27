#Requires -Version 7.0
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
Set-Location (Split-Path $PSScriptRoot -Parent)
npm run build
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace --all-targets
$metadata = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
$cargoVersion = ($metadata.packages | Where-Object name -EQ 'wattdrive-desktop').version
$tauriVersion = (Get-Content src-tauri/tauri.conf.json -Raw | ConvertFrom-Json).version
$npmVersion = (Get-Content package.json -Raw | ConvertFrom-Json).version
if ((-not $cargoVersion) -or $cargoVersion -ne $tauriVersion -or $cargoVersion -ne $npmVersion) {
    throw 'Version mismatch or missing version in application manifests.'
}
npx tauri build --debug --no-bundle
node scripts/e2e.mjs
