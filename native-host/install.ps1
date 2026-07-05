# Topos native host installer for Windows.
# PowerShell mirror of ../install.sh, compatible with Windows PowerShell 5.1
# (stock Windows 10/11, no pwsh required).
#
# Usage (default execution policy blocks unsigned scripts, so launch with):
#   powershell -ExecutionPolicy Bypass -File install.ps1
#   powershell -ExecutionPolicy Bypass -File install.ps1 -ChromeExtensionId <32-char-id>
#
# Every step is per-user (LOCALAPPDATA + HKCU) and needs no elevation, except
# the firewall rule. If the shell is not elevated, the script prints the exact
# command to run in an Administrator PowerShell instead of failing.
#
# This file is intentionally pure ASCII: PowerShell 5.1 reads BOM-less .ps1
# files as ANSI and would mangle any non-ASCII character.

param(
    # Chrome extension ID (the 32-char hash shown at chrome://extensions).
    # Optional: when omitted, the Chrome manifest keeps the
    # REPLACE_WITH_EXTENSION_HASH placeholder and a NOTE says which file to edit.
    [string]$ChromeExtensionId
)

$ErrorActionPreference = 'Stop'

# This script lives inside native-host/, so $PSScriptRoot is the crate root.
$NativeDir  = $PSScriptRoot
$InstallDir = Join-Path $env:LOCALAPPDATA 'Topos'
$BinaryDest = Join-Path $InstallDir 'topos-host.exe'

Write-Host '==> Building native host...'
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Host 'ERROR: cargo not found on PATH. Install Rust from https://rustup.rs and re-run.'
    exit 1
}
Push-Location $NativeDir
try {
    cargo build --release
    # $ErrorActionPreference = 'Stop' does not catch native command failures
    # in PowerShell 5.1, so check the exit code explicitly.
    if ($LASTEXITCODE -ne 0) {
        Write-Host "ERROR: cargo build failed (exit code $LASTEXITCODE)."
        exit 1
    }
} finally {
    Pop-Location
}

Write-Host "==> Installing binary to $BinaryDest..."
# New-Item -Force is idempotent: it succeeds if the directory already exists.
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
$BuiltBinary = Join-Path $NativeDir 'target\release\topos-host.exe'
if (-not (Test-Path $BuiltBinary)) {
    Write-Host "ERROR: built binary not found at $BuiltBinary."
    exit 1
}
Copy-Item -Force $BuiltBinary $BinaryDest

Write-Host '==> Installing native host manifests...'

# Manifests are generated from $BinaryDest so the path can never drift from
# the actual binary location (copying a hardcoded JSON caused BUG-006 on
# Linux). ConvertTo-Json also escapes the backslashes in the Windows path,
# which a hand-templated here-string would get wrong.
#
# On Windows, browsers do not scan a directory for native messaging manifests
# the way they do on Linux: they read a registry key under HKCU whose default
# value is the full path to the manifest JSON file.

# --- Firefox ---
$FirefoxManifestPath = Join-Path $InstallDir 'com.topos.cast.firefox.json'
$FirefoxManifest = [ordered]@{
    name               = 'com.topos.cast'
    description        = 'Topos Native Host'
    path               = $BinaryDest
    type               = 'stdio'
    allowed_extensions = @('topos@cast')
}
$FirefoxJson = ConvertTo-Json -InputObject $FirefoxManifest
# WriteAllText writes UTF-8 without BOM. Set-Content -Encoding UTF8 in
# PowerShell 5.1 prepends a BOM, which browsers may reject when parsing.
[System.IO.File]::WriteAllText($FirefoxManifestPath, $FirefoxJson)

$FirefoxKey = 'HKCU:\Software\Mozilla\NativeMessagingHosts\com.topos.cast'
if (-not (Test-Path $FirefoxKey)) {
    New-Item -Path $FirefoxKey -Force | Out-Null
}
Set-ItemProperty -Path $FirefoxKey -Name '(Default)' -Value $FirefoxManifestPath
Write-Host "    Firefox: $FirefoxKey -> $FirefoxManifestPath"

# --- Chrome / Chromium ---
$ChromePlaceholder = 'REPLACE_WITH_EXTENSION_HASH'
if (-not $ChromeExtensionId) {
    $ChromeExtensionId = $ChromePlaceholder
}
$ChromeManifestPath = Join-Path $InstallDir 'com.topos.cast.chrome.json'
$ChromeManifest = [ordered]@{
    name            = 'com.topos.cast'
    description     = 'Topos Native Host'
    path            = $BinaryDest
    type            = 'stdio'
    allowed_origins = @("chrome-extension://$ChromeExtensionId/")
}
$ChromeJson = ConvertTo-Json -InputObject $ChromeManifest
[System.IO.File]::WriteAllText($ChromeManifestPath, $ChromeJson)

# One manifest file can back several registry keys. Creating a key for a
# browser that is not installed is harmless and makes the install
# order-independent (a browser installed later still finds the host).
$ChromiumKeys = @(
    'HKCU:\Software\Google\Chrome\NativeMessagingHosts\com.topos.cast',
    'HKCU:\Software\Chromium\NativeMessagingHosts\com.topos.cast'
)
foreach ($Key in $ChromiumKeys) {
    if (-not (Test-Path $Key)) {
        New-Item -Path $Key -Force | Out-Null
    }
    Set-ItemProperty -Path $Key -Name '(Default)' -Value $ChromeManifestPath
    Write-Host "    Chrome/Chromium: $Key -> $ChromeManifestPath"
}
if ($ChromeExtensionId -eq $ChromePlaceholder) {
    Write-Host "    NOTE: edit $ChromeManifestPath and replace $ChromePlaceholder"
    Write-Host '          with the extension ID shown at chrome://extensions,'
    Write-Host '          or re-run this script with -ChromeExtensionId <id>.'
}

Write-Host ''
Write-Host '==> Firewall...'
# The native host runs a local HLS proxy on TCP 7070 that the Chromecast must
# reach. Do not rely on the automatic first-listen firewall popup: the host
# runs headless under the browser, so the prompt may never be seen.
# -RemoteAddress LocalSubnet matches the machine's current local subnet on
# every adapter, so no hardcoded 192.168.x.x range is needed.
# New-NetFirewallRule persists across reboots by default: no save step needed.
$RuleName = 'Topos proxy'
# Profile Private,Domain (not just Private): Windows classifies most new
# networks as Public by default, and a Private-only rule silently never
# matches there, so casting would fail with no error.
$FirewallCmd = "New-NetFirewallRule -DisplayName '$RuleName' -Direction Inbound" +
    " -Protocol TCP -LocalPort 7070 -Action Allow -Profile Private,Domain" +
    " -RemoteAddress LocalSubnet -Program '$BinaryDest'"
# Detect (don't just document) whether the active network is even covered by
# the rule's Private,Domain profiles. Public is Windows' default classification
# for a freshly joined network, so this is the common case, not an edge case -
# check now and name the actual interface so the fix below is copy-pasteable.
$PublicProfiles = Get-NetConnectionProfile | Where-Object { $_.NetworkCategory -eq 'Public' }
foreach ($p in $PublicProfiles) {
    Write-Host "    WARNING: network '$($p.Name)' is classified Public - the firewall rule below will NOT match it, and casting will fail silently."
    Write-Host "    Fix (elevated PowerShell): Set-NetConnectionProfile -InterfaceAlias '$($p.InterfaceAlias)' -NetworkCategory Private"
}

$ExistingRule = Get-NetFirewallRule -DisplayName $RuleName -ErrorAction SilentlyContinue
if ($ExistingRule) {
    Write-Host '    Firewall rule already present.'
} else {
    $IsAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
        ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    if ($IsAdmin) {
        try {
            New-NetFirewallRule -DisplayName $RuleName -Direction Inbound `
                -Protocol TCP -LocalPort 7070 -Action Allow -Profile Private,Domain `
                -RemoteAddress LocalSubnet -Program $BinaryDest | Out-Null
            Write-Host '    Firewall rule added: local subnet -> TCP 7070 (persistent).'
        } catch {
            Write-Host "    WARNING: could not add the firewall rule: $($_.Exception.Message)"
            Write-Host '    Run this in an elevated (Administrator) PowerShell:'
            Write-Host "      $FirewallCmd"
        }
    } else {
        Write-Host '    Not running as Administrator: skipping the firewall rule.'
        Write-Host '    Casting will fail until TCP 7070 is reachable from the Chromecast.'
        Write-Host '    Run this in an elevated (Administrator) PowerShell:'
        Write-Host "      $FirewallCmd"
    }
}

Write-Host ''
Write-Host 'Done. Load the extension in Firefox:'
Write-Host '  about:debugging -> This Firefox -> Load Temporary Add-on -> extension/manifest.json'
