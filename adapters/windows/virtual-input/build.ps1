# SPDX-License-Identifier: MIT
# Builds unsigned files only. Never installs, loads, signs or changes host policy.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = $PSScriptRoot
$build = Join-Path $root 'build'
$driver = Join-Path $root 'driver'
$packages = Join-Path $driver 'packages'
$kit = '10.0.26100.6584'
function Require-Reserve {
    $free = [IO.DriveInfo]::new([IO.Path]::GetPathRoot($root)).AvailableFreeSpace
    Write-Host "Available bytes: $free"
    if ($free -lt 10737418240) { throw 'BLOCKED: less than 10737418240 free bytes' }
}
function Run-Checked([string] $program, [string[]] $arguments) {
    & $program @arguments
    if ($LASTEXITCODE -ne 0) { throw "$program failed: $LASTEXITCODE" }
}
Require-Reserve
New-Item -ItemType Directory -Force $build | Out-Null
try {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
    $vs = & $vswhere -latest -products '*' -version '[17.14,17.15)' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if ($LASTEXITCODE -ne 0 -or !$vs) { throw 'BLOCKED: existing VS2022 17.14 C++ tools required' }
    $msvc = (Get-Content (Join-Path $vs 'VC/Auxiliary/Build/Microsoft.VCToolsVersion.default.txt')).Trim()
    if (!$msvc.StartsWith('14.44.')) { throw "BLOCKED: MSVC 14.44 required, found $msvc" }
    # The supported PowerShell launcher avoids cmd.exe quoting/native argument conversion.
    $devshell = Join-Path $vs 'Common7/Tools/Launch-VsDevShell.ps1'
    & $devshell -Arch amd64 -HostArch amd64 -SkipAutomaticLocation
    if (!$env:VCToolsVersion -or $env:VCToolsVersion.TrimEnd('\') -ne $msvc) {
        throw 'VS developer environment did not select the pinned MSVC version'
    }
    $tools = [ordered]@{
        vs = (Get-Item (Join-Path $vs 'Common7/IDE/devenv.exe')).VersionInfo.ProductVersion
        msvc = $msvc; sdk = $kit; wdk = $kit
        runnerImage = $env:ImageVersion
        sourceHead = (& git rev-parse HEAD).Trim()
        evidence = 'unsigned build and queue model only; native execution BLOCKED'
    }
    $tools | ConvertTo-Json | Set-Content (Join-Path $build 'toolchain.json') -Encoding utf8
    Run-Checked 'nuget' @('restore', (Join-Path $driver 'packages.config'), '-PackagesDirectory', $packages,
        '-Source', 'https://api.nuget.org/v3/index.json', '-NonInteractive', '-NoCache', '-DirectDownload')
    Require-Reserve
    $msbuild = Join-Path $vs 'MSBuild/Current/Bin/MSBuild.exe'
    Run-Checked $msbuild @((Join-Path $driver 'MixyardIngress.vcxproj'), '/m:2',
        '/p:Configuration=Release', '/p:Platform=x64', "/p:VCToolsVersion=$msvc",
        '/fl', "/flp:logfile=$build/msbuild.log;verbosity=normal")
    Require-Reserve
    Run-Checked 'cl.exe' @('/nologo', '/std:c++17', '/W4', '/WX', '/EHsc',
        "$root/../../../verification/windows-input/transport_check.cpp", "/Fe:$build/transport-check.exe", "/Fo:$build/transport-check.obj")
    Run-Checked (Join-Path $build 'transport-check.exe') @()
    $package = Join-Path $build 'unsigned'
    New-Item -ItemType Directory -Force $package | Out-Null
    Copy-Item (Join-Path $driver 'build/x64/Release/MixyardIngress.sys') $package -Force
    Copy-Item (Join-Path $driver 'MixyardIngress.inf') $package -Force
    Copy-Item (Join-Path $root '../../../LICENSE') $package -Force
    $wdk = Join-Path $packages "Microsoft.Windows.WDK.x64.$kit/c"
    $infverif = Join-Path $wdk 'tools/10.0.26100.0/x64/infverif.exe'
    $inf2cat = Join-Path $wdk 'bin/10.0.26100.0/x86/Inf2Cat.exe'
    Run-Checked $infverif @('/w', '/v', (Join-Path $package 'MixyardIngress.inf'))
    Run-Checked $inf2cat @("/driver:$package", '/os:10_GE_X64', '/verbose')
    Run-Checked 'python' @("$root/../../../verification/windows-input/artifact_check.py", $package)
} finally {
    Require-Reserve
}
