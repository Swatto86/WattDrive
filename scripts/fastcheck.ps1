#Requires -Version 7.0
[CmdletBinding()]
param([string]$Package)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
Set-Location (Split-Path $PSScriptRoot -Parent)
cargo fmt --all -- --check
if ($Package) {
    cargo check --locked -p $Package --all-targets
} else {
    npx tsc --noEmit
    cargo clippy --locked --workspace --all-targets -- -D warnings
}
