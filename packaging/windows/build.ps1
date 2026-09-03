# SPDX-FileCopyrightText: 2026 Curtis Galloway
# SPDX-License-Identifier: Apache-2.0

# Stage the Windows payload and build the per-user MSI (WiX v7).
#
#   packaging\windows\build.ps1 -Version 0.3.1 `
#       -Exe target\x86_64-pc-windows-msvc\release\qbranch.exe -OutDir dist
#
# Requires the WiX CLI:  dotnet tool install --global wix
# (and `wix eula accept wix7` once per machine, or the first build dies with
# WIX7015, the Open Source Maintenance Fee EULA, whose fee applies only to
# consumers generating revenue, which this is not).
#
# The staged layout is the same bin-beside-share prefix the .deb and the
# tarball install: bin\qbranch.exe, share\qbranch\skills\<name>\SKILL.md,
# doc\LICENSE, doc\README.md. Sign the exe before calling this, so the
# signature travels inside the MSI; the MSI itself is signed afterwards.

param(
    [Parameter(Mandatory = $true)][string]$Version,
    [Parameter(Mandatory = $true)][string]$Exe,
    [string]$OutDir = "dist"
)

$ErrorActionPreference = "Stop"

$Windows = $PSScriptRoot
$Repo = (Resolve-Path (Join-Path $Windows "..\..")).Path

# ProductVersion has to be three numeric fields. A tag build already is one;
# a workflow_dispatch build is something like 0.0.0~dev.1a2b3c4, which msi
# cannot express, so it becomes 0.0.0 and says so. The binary's own version
# is unaffected either way: it is compiled in, not passed here.
if ($Version -match '^\d+\.\d+\.\d+$') {
    $MsiVersion = $Version
} else {
    $MsiVersion = '0.0.0'
    Write-Host "::warning::'$Version' is not a three-field MSI version; building as $MsiVersion"
}

# Package.wxs lists the skills one file each, because WiX has no glob. Refuse
# to build when the checkout has a skill the package would silently leave
# out, or lists one the checkout no longer has. Keep this list and the
# components in Package.wxs in step.
$KnownSkills = @("review-plugins", "agent-audit")
$Skills = Get-ChildItem (Join-Path $Repo "skills") -Directory | Select-Object -ExpandProperty Name
$missing = @($Skills | Where-Object { $KnownSkills -notcontains $_ })
if ($missing.Count -gt 0) {
    throw "skills\ holds skills Package.wxs does not know: $($missing -join ', '). Add a component for each."
}
$gone = @($KnownSkills | Where-Object { $Skills -notcontains $_ })
if ($gone.Count -gt 0) {
    throw "Package.wxs lists skills the checkout no longer has: $($gone -join ', ')."
}

if (-not (Test-Path $Exe)) { throw "no executable at $Exe" }
$Exe = (Resolve-Path $Exe).Path

if (-not (Test-Path $OutDir)) { New-Item -ItemType Directory -Force $OutDir | Out-Null }
$OutDir = (Resolve-Path $OutDir).Path
$Stage = Join-Path $OutDir "stage"
if (Test-Path $Stage) { Remove-Item -Recurse -Force $Stage }

$BinDir = Join-Path $Stage "bin"
$SkillsDir = Join-Path $Stage "share\qbranch\skills"
$DocDir = Join-Path $Stage "doc"
foreach ($dir in @($BinDir, $SkillsDir, $DocDir)) {
    New-Item -ItemType Directory -Force $dir | Out-Null
}

Copy-Item $Exe (Join-Path $BinDir "qbranch.exe")
foreach ($skill in $KnownSkills) {
    $dir = Join-Path $SkillsDir $skill
    New-Item -ItemType Directory -Force $dir | Out-Null
    Copy-Item (Join-Path $Repo "skills\$skill\SKILL.md") (Join-Path $dir "SKILL.md")
}
Copy-Item (Join-Path $Repo "LICENSE") (Join-Path $DocDir "LICENSE")
Copy-Item (Join-Path $Repo "README.md") (Join-Path $DocDir "README.md")

# -arch x64: the package carries an x86_64 executable. Nothing lands in a
# Program Files directory either way, because the install is per-user under
# LocalAppData.
$Msi = Join-Path $OutDir "qbranch-v$Version-x86_64-pc-windows-msvc.msi"
& wix build -arch x64 `
    -d "Version=$MsiVersion" -d "StageDir=$Stage" `
    (Join-Path $Windows "Package.wxs") -o $Msi
if ($LASTEXITCODE -ne 0) { throw "wix build failed" }

Remove-Item -Recurse -Force $Stage
Write-Host "built $Msi"
