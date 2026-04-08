# daemon/scripts/devshell.ps1
#
# Activates the Visual Studio BuildTools 2022 dev shell and positions
# the prompt at the daemon directory. After running this, `cargo build`
# inside `daemon/` can compile crates that need cmake + MSVC (notably
# llama-cpp-2, which builds llama.cpp from source via cmake during its
# build script).
#
# Usage:
#     .\daemon\scripts\devshell.ps1
#
# Or from outside the repo (e.g. as a Windows Terminal startup command):
#     powershell -NoExit -File C:\path\to\lupus\daemon\scripts\devshell.ps1
#
# See docs/DAEMON_DEV_SETUP.md for the full prerequisite list and
# troubleshooting.

# Use 'Continue' (not 'Stop') because Windows PowerShell 5.1 promotes
# native-command stderr writes to terminating errors under 'Stop', and
# tools like cl.exe legitimately print their version banner to stderr.
$ErrorActionPreference = 'Continue'

# Hardcoded launcher path. The wider Lupus dev environment depends on
# Visual Studio BuildTools 2022 being installed at the standard MS path.
# If you installed BuildTools to a different location, edit the path below.
$BuildToolsLauncher = 'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\Common7\Tools\Launch-VsDevShell.ps1'

if (-not (Test-Path $BuildToolsLauncher)) {
    Write-Host "ERROR: VS BuildTools 2022 dev shell launcher not found at:" -ForegroundColor Red
    Write-Host "  $BuildToolsLauncher" -ForegroundColor Red
    Write-Host ""
    Write-Host "If BuildTools is installed elsewhere, edit this script and update" -ForegroundColor Red
    Write-Host "`$BuildToolsLauncher to point at your install." -ForegroundColor Red
    Write-Host "If you don't have BuildTools at all, see docs/DAEMON_DEV_SETUP.md." -ForegroundColor Red
    return
}

# Activate the dev shell. The launcher's underlying VsDevCmd.bat prints a
# cosmetic "'vswhere.exe' is not recognized" warning from a sub-cmd.exe
# child process. That stderr leaks past PowerShell stream redirection
# because it's written directly to the console handle. We accept the noise.
& $BuildToolsLauncher -Arch amd64 -SkipAutomaticLocation 2>&1 | Out-Null

# bindgen (used by llama-cpp-sys-2 at build time) needs libclang.dll to
# parse llama.cpp's C headers. The LLVM Windows installer doesn't add its
# bin dir to PATH by default, so bindgen can't find it without help.
# We set LIBCLANG_PATH explicitly to point at wherever libclang lives.
$LibClangCandidates = @(
    'C:\Program Files\LLVM\bin\libclang.dll',
    'D:\Program Files\LLVM\bin\libclang.dll',
    'D:\Tools\LLVM\bin\libclang.dll',
    'C:\Program Files (x86)\LLVM\bin\libclang.dll'
)
$LibClang = $LibClangCandidates | Where-Object { Test-Path $_ } | Select-Object -First 1
if ($LibClang) {
    $env:LIBCLANG_PATH = Split-Path -Parent $LibClang
}

# Position at the daemon dir, resolved relative to this script's own
# location so the wrapper works regardless of the caller's CWD.
$DaemonDir = Resolve-Path (Join-Path $PSScriptRoot '..')
Set-Location $DaemonDir

# Sanity check: are the build tools actually reachable?
$Required = @('cl', 'cmake', 'cargo', 'rustc')
$Missing = $Required | Where-Object { -not (Get-Command $_ -ErrorAction SilentlyContinue) }

if ($Missing.Count -gt 0) {
    Write-Warning "Missing from PATH after dev shell activation: $($Missing -join ', ')"
    Write-Warning "The daemon build will likely fail. Re-check your VS BuildTools install."
    Write-Warning "See docs/DAEMON_DEV_SETUP.md for troubleshooting."
    return
}

# Pretty-print the toolchain versions so it's obvious at a glance that
# everything is wired up correctly.
$ClBanner    = (& cl.exe 2>&1 | Select-Object -First 1).ToString()
$ClVersion   = if ($ClBanner -match 'Version (\S+)') { $Matches[1] } else { '?' }
$CmakeBanner = (& cmake --version 2>&1 | Select-Object -First 1).ToString()
$CmakeVersion = if ($CmakeBanner -match '(\d+\.\d+\.\S+)') { $Matches[1] } else { '?' }
$RustcBanner  = (& rustc --version 2>&1).ToString()
$RustcVersion = if ($RustcBanner -match '(\d+\.\d+\.\S+)') { $Matches[1] } else { '?' }

Write-Host ''
Write-Host 'lupus-daemon dev shell ready' -ForegroundColor Green
Write-Host "  cl.exe   : $ClVersion"
Write-Host "  cmake    : $CmakeVersion"
Write-Host "  rustc    : $RustcVersion"
if ($env:LIBCLANG_PATH) {
    Write-Host "  libclang : $env:LIBCLANG_PATH"
} else {
    Write-Warning "libclang not found. bindgen will fail. See docs/DAEMON_DEV_SETUP.md"
}
Write-Host "  cwd      : $DaemonDir"
Write-Host ''
Write-Host 'Quick start:' -ForegroundColor Cyan
Write-Host '  cargo check          # validate without compiling'
Write-Host '  cargo build          # debug build'
Write-Host '  cargo build --release'
Write-Host '  cargo run            # run the daemon'
Write-Host ''
