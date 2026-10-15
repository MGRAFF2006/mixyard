# SPDX-License-Identifier: MIT
# Existing SDK only. Compile/link + injected checks; NEVER call device APIs.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$repo = (Resolve-Path (Join-Path $PSScriptRoot '../../../..')).Path
$run = if ($env:GITHUB_RUN_ID) { "$env:GITHUB_RUN_ID-$env:GITHUB_RUN_ATTEMPT" } else { [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssfffffffZ') }
$build = Join-Path $PSScriptRoot "build/$run"
function Require-Reserve {
    $free = [IO.DriveInfo]::new([IO.Path]::GetPathRoot($PSScriptRoot)).AvailableFreeSpace
    Write-Host "Available bytes: $free"
    if ($free -lt 10737418240) { throw 'BLOCKED: less than 10737418240 free bytes' }
}
function Run-Checked([string] $program, [string[]] $arguments, [string] $log) {
    & $program @arguments 2>&1 | Tee-Object -FilePath (Join-Path $build $log)
    if ($LASTEXITCODE -ne 0) { throw "$program failed: $LASTEXITCODE" }
}
Require-Reserve
if (Test-Path -LiteralPath $build) { throw 'Refusing to overwrite prior build evidence' }
New-Item -ItemType Directory $build | Out-Null
$evidence = [ordered]@{
    schemaVersion = 1
    sourceHead = (& git -C $repo rev-parse HEAD).Trim()
    sourceTree = (& git -C $repo rev-parse 'HEAD^{tree}').Trim()
    runId = $env:GITHUB_RUN_ID; runAttempt = $env:GITHUB_RUN_ATTEMPT
    event = $env:GITHUB_EVENT_NAME; runnerImage = $env:ImageVersion
    status = 'incomplete'
    execution = 'injected operations only; NEVER actual SwDevice/CreateFile/IOCTL/audio calls'
    nativeDeviceExecution = 'BLOCKED'; mmDeviceMapping = 'BLOCKED'; consumerPCM = 'BLOCKED'
    productionApply = 'disabled'
}
try {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
    $vs = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if ($LASTEXITCODE -ne 0 -or !$vs) { throw 'Existing VS C++ tools required' }
    & (Join-Path $vs 'Common7/Tools/Launch-VsDevShell.ps1') -Arch amd64 -HostArch amd64 -SkipAutomaticLocation
    $evidence.vs = (Get-Item (Join-Path $vs 'Common7/IDE/devenv.exe')).VersionInfo.ProductVersion
    $evidence.msvc = $env:VCToolsVersion
    $evidence.sdk = $env:WindowsSDKVersion
    $evidence.compiler = (Get-Command cl.exe).Source
    $sdkLib = Join-Path $env:WindowsSdkDir "Lib/$($env:WindowsSDKVersion.TrimEnd('\'))/um/x64/Swdevice.lib"
    if (!(Test-Path -LiteralPath $sdkLib -PathType Leaf)) { throw 'Real SDK Swdevice.lib missing' }
    $evidence.swdeviceLibrarySHA256 = (Get-FileHash -Algorithm SHA256 $sdkLib).Hash.ToLowerInvariant()
    $evidence.sources = [ordered]@{}
    $sources = & git -C $repo ls-files -- adapters/windows/virtual-input/producer verification/windows-input/producer `
        adapters/windows/virtual-input/transport.hpp adapters/windows/virtual-input/driver/ioctl.hpp `
        adapters/windows/virtual-input/capture/control.hpp adapters/windows/virtual-input/capture/status.hpp `
        adapters/windows/virtual-input/capture/delivery.hpp adapters/windows/virtual-input/capture/MixyardCapture.inf `
        .github/workflows/windows-input.yml LICENSE
    if ($LASTEXITCODE -ne 0) { throw 'Source inventory failed' }
    foreach ($source in $sources) {
        $evidence.sources[$source] = (Get-FileHash -Algorithm SHA256 (Join-Path $repo $source)).Hash.ToLowerInvariant()
    }
    $exe = Join-Path $build 'mixyard-producer.exe'
    $arguments = @('/nologo', '/std:c++17', '/W4', '/WX', '/permissive-', '/EHsc', '/utf-8', '/O2', '/MT',
        '/DUNICODE', '/D_UNICODE', '/DNOMINMAX', '/DWIN32_LEAN_AND_MEAN', '/D_WIN32_WINNT=0x0A00',
        (Join-Path $PSScriptRoot 'main.cpp'), (Join-Path $repo 'verification/windows-input/producer/check.cpp'),
        "/Fe:$exe", "/Fo:$build/", '/link', 'Swdevice.lib', '/INCREMENTAL:NO')
    $evidence.compileArguments = $arguments
    Run-Checked 'cl.exe' $arguments 'compile.log'
    Require-Reserve
    Run-Checked $exe @('--selfcheck') 'injected-selfcheck.log'
    & $exe 2>&1 | Out-File (Join-Path $build 'no-arguments.log')
    if ($LASTEXITCODE -ne 2) { throw 'No-arguments safety check failed' }
    & $exe guest-provision 2>&1 | Out-File (Join-Path $build 'missing-opt-in.log')
    if ($LASTEXITCODE -ne 2) { throw 'Missing-opt-in safety check failed' }
    Run-Checked 'dumpbin.exe' @('/headers', '/imports', $exe) 'pe-imports.log'
    Copy-Item (Join-Path $repo 'LICENSE') $build
    $evidence.status = 'passed-compile-link-and-injected-checks-only'
} finally {
    $evidence.artifacts = [ordered]@{}
    foreach ($file in (Get-ChildItem -LiteralPath $build -File)) {
        $evidence.artifacts[$file.Name] = @{ bytes = $file.Length; sha256 = (Get-FileHash -Algorithm SHA256 $file.FullName).Hash.ToLowerInvariant() }
    }
    $evidence | ConvertTo-Json -Depth 10 | Set-Content (Join-Path $build 'evidence.json') -Encoding utf8
    Require-Reserve
}
