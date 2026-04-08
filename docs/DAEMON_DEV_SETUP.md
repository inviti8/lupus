# Lupus Daemon Dev Setup (Windows)

How to set up a Windows machine to build and develop the `lupus-daemon`
Rust crate. The daemon links a Rust binding (`llama-cpp-2`) to the
upstream llama.cpp C++ library, which is built from source by Cargo via
`cmake`. That makes the C++ toolchain a hard prerequisite, not an optional
extra.

This document covers the toolchain we standardized on, why we picked it,
the wrapper script that makes day-to-day dev one command, and the known
quirks worth not chasing as bugs.

For Linux/macOS dev, the equivalent setup is `apt install build-essential
cmake` (or `xcode-select --install` plus Homebrew `cmake`) — there's no
unique Windows weirdness on those platforms. The rest of this doc is
Windows-specific.

---

## Prerequisites

| Component | Version we use | Why |
|---|---|---|
| Windows | 10 or 11 (we test on 11 24H2) | base OS |
| Visual Studio Build Tools 2022 | 17.x with the C++ workload | provides `cl.exe`, `link.exe`, `lib.exe`, `nmake.exe`, the Windows SDK headers/libs, and the bundled CMake at `Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe` |
| LLVM | 18.x or later (we use 22.1.2) | provides `libclang.dll` for `bindgen`, which `llama-cpp-sys-2`'s build script uses to generate Rust FFI bindings from llama.cpp's C headers. Without this the daemon build fails at the first `cargo check` with `Unable to find libclang`. Install via `winget install -e --id LLVM.LLVM` |
| Rust toolchain | 1.93.0 or later (any recent stable) | `rustup default stable` |
| Git | any | repo clone |

We deliberately do **not** require:

- The full Visual Studio IDE — Build Tools is enough
- A separate CMake install — the one bundled with Build Tools is sufficient
- A separate Windows SDK install — Build Tools includes one
- WSL2 — the daemon builds natively on Windows

## How to verify what's installed

Open any PowerShell window and run:

```powershell
& 'C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe' -all -prerelease -format json | ConvertFrom-Json | ForEach-Object { $_.installationPath }
```

If Visual Studio Build Tools 2022 is installed, you'll see one of these
in the output:

```
C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools
```

Note that `vswhere.exe` does **not** always list Build Tools alongside
Community/Professional/Enterprise — depending on your install history it
may only show the IDE products. The path-existence check is more reliable
than `vswhere`. If the directory exists at `C:\Program Files (x86)\Microsoft
Visual Studio\2022\BuildTools` you have Build Tools installed.

To verify the C++ workload is present (not just the Build Tools shell):

```powershell
Test-Path 'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC'
Test-Path 'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe'
```

Both should return `True`.

## How to install (if missing)

1. Download the bootstrapper from
   <https://visualstudio.microsoft.com/downloads/?q=build+tools>
2. Run `vs_BuildTools.exe`. The GUI opens.
3. On the Workloads tab, check **"Desktop development with C++"**. The
   right-hand panel shows the included components — leave the defaults
   (which include `MSVC v143`, `Windows 11 SDK`, and `C++ CMake tools for
   Windows`).
4. Optionally change the Install location on the Installation locations
   tab — the default is `C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools`
   which is fine.
5. Click Install. ~3-5 GB download, ~5-15 min depending on network.

You can also do a fully scripted install:

```cmd
vs_BuildTools.exe ^
  --add Microsoft.VisualStudio.Workload.VCTools ^
  --includeRecommended ^
  --quiet --norestart --wait
```

The `Microsoft.VisualStudio.Workload.VCTools` workload bundles MSVC + the
Windows SDK + the bundled CMake automatically. `--includeRecommended`
adds the recommended optional components. The flags `--quiet --norestart
--wait` make the install fully unattended.

## The daily dev workflow

### One-time: install Rust

```powershell
winget install -e --id Rustlang.Rustup
rustup default stable
```

### Every day: open the dev shell

From any fresh PowerShell window inside the repo:

```powershell
.\daemon\scripts\devshell.ps1
```

This script (see `daemon/scripts/devshell.ps1`) does three things:

1. Activates the VS Build Tools 2022 dev shell, which puts `cl.exe`,
   `cmake`, `link`, `lib`, `nmake` and the Windows SDK headers/libs on
   `PATH` / `INCLUDE` / `LIB` / `LIBPATH`.
2. `cd`s into `daemon/`.
3. Prints a one-line banner with the cl/cmake/rustc versions, so you can
   see at a glance that the toolchain is wired up correctly.

After it runs you can `cargo check`, `cargo build`, `cargo run`, etc.
directly. The dev shell environment persists for the lifetime of that
PowerShell session.

If you use Windows Terminal, you can pin a profile that runs the dev
shell as its startup command. Add this to your Windows Terminal
`settings.json` profile list:

```json
{
    "name": "Lupus Daemon",
    "commandline": "powershell -NoExit -File D:\\repos\\lupus\\daemon\\scripts\\devshell.ps1",
    "startingDirectory": "D:\\repos\\lupus\\daemon",
    "icon": "D:\\repos\\lupus\\dist\\icon.png"
}
```

Now opening a "Lupus Daemon" tab drops you straight into a ready-to-build
shell.

## Known quirks (don't chase these as bugs)

### `'vswhere.exe' is not recognized as an internal or external command`

When you run `devshell.ps1` (or activate the dev shell manually) you'll
see this warning printed before the version banner:

```
'vswhere.exe' is not recognized as an internal or external command,
operable program or batch file.
```

This is **harmless and expected**. It comes from a child `cmd.exe`
process inside `Launch-VsDevShell.ps1`'s implementation that tries to
invoke `vswhere.exe` from a hardcoded relative path that doesn't exist
in the current shell. The activation still succeeds via a different
code path. We can't suppress the warning because it's emitted by a
grandchild process writing directly to the console handle, bypassing
PowerShell's stream redirection.

If you ever see the warning and the version banner doesn't appear after
it, that's a real failure — but the warning alone is fine.

### `vswhere -all` may not list Build Tools

`vswhere.exe -all -prerelease` enumerates installed VS instances, but
**Build Tools 2022 is sometimes missing from the output** even when it's
clearly installed (we observed this on a machine where Community 2022
showed up but BuildTools 2022 didn't, despite both being present at the
expected paths). Check the install path directly with `Test-Path` rather
than relying on `vswhere`.

### Native command stderr → PowerShell errors

`cl.exe` prints its version banner to stderr by convention, and Windows
PowerShell 5.1 will treat that as a terminating error if
`$ErrorActionPreference` is set to `'Stop'` in the current scope. If you
write your own helper script, set `$ErrorActionPreference = 'Continue'`
or wrap the native call in a `try`/`catch`. PowerShell 7.3+ has the
`$PSNativeCommandUseErrorActionPreference` toggle for this; PS 5.1 does
not. The `devshell.ps1` script handles this correctly.

### `cargo build` first-time cost

The first `cargo build` after adding `llama-cpp-2` (or any other
cmake-driven crate) takes 5-15 minutes because llama.cpp compiles from
source under `target/debug/build/`. Subsequent builds are incremental
and take 5-30 seconds. If the first build appears stuck, check Task
Manager for `cl.exe` processes — they're usually running flat-out on
all cores.

## Where the trained models live

The daemon needs two model files to run inference:

| File | Size | Source | Local path |
|---|---|---|---|
| TinyAgent base GGUF | 637 MB | `squeeze-ai-lab/TinyAgent-1.1B-GGUF` on HF | `dist/tinyagent/TinyAgent-1.1B-Q4_K_M.gguf` |
| Lupus search LoRA (GGUF) | 9 MB | trained by us, `s3://b51y8tev3v/models/lupus-tinyagent/final/` | `dist/lupus-tinyagent-search/adapter.gguf` |

Both are gitignored. To set them up on a fresh clone:

```powershell
# Base GGUF (one-time, ~3 min)
hf download squeeze-ai-lab/TinyAgent-1.1B-GGUF TinyAgent-1.1B-Q4_K_M.gguf --local-dir dist/tinyagent

# Search LoRA from S3 (needs .env with S3 credentials)
# See docs/TINYAGENT_STEPC_FINDINGS.md for the direct-download script
# (RunPod's S3 recursive list is broken; pull files by exact key)
```

Once both files are present, the daemon can load them on startup:

```rust
// daemon/src/agent.rs (after Phase 1)
let backend = LlamaBackend::init()?;
let model = LlamaModel::load_from_file(&backend, model_path, model_params)?;
let lora = model.lora_adapter_init(&search_adapter_path)?;
```

## Troubleshooting

### `error: linker 'link.exe' not found`

You're not in the dev shell. Run `.\daemon\scripts\devshell.ps1` first.

### `cmake: command not found`

Same as above — `cmake` is provided by the dev shell, not the system.
Outside the dev shell, `cmake` won't be on PATH.

### `cargo build` fails with `failed to run custom build command for llama-cpp-sys-2`

Check the build log for the actual cmake error. Common causes:

- Build Tools install is incomplete (missing C++ workload). Run the
  installer again and add the workload.
- Old CMake from a separate install is shadowing the BuildTools one.
  Run `Get-Command cmake` and confirm it points at the BuildTools path.
- Antivirus interfering with the build. Add an exclusion for
  `target/debug/build/llama-cpp-sys-2-*/` if so.

### `Unable to find libclang` during cargo check / build

The LLVM Windows installer doesn't add `C:\Program Files\LLVM\bin` to
system PATH by default, so bindgen can't find `libclang.dll` even after
LLVM is installed. The dev shell wrapper (`daemon/scripts/devshell.ps1`)
auto-detects the LLVM install at common paths and sets `LIBCLANG_PATH`
explicitly — but only if you launched your shell via the wrapper.

If you're running `cargo` outside the wrapper, set the env var manually:

```powershell
$env:LIBCLANG_PATH = 'C:\Program Files\LLVM\bin'
```

Or persist it for all future shells:

```powershell
[System.Environment]::SetEnvironmentVariable('LIBCLANG_PATH', 'C:\Program Files\LLVM\bin', 'User')
```

If LLVM isn't installed at all, install it: `winget install -e --id LLVM.LLVM`.

### `Launch-VsDevShell.ps1 : ... cannot be loaded because running scripts is disabled`

Your PowerShell ExecutionPolicy is too strict. Either run the wrapper
with bypass:

```powershell
powershell -ExecutionPolicy Bypass -File .\daemon\scripts\devshell.ps1
```

Or relax the policy permanently:

```powershell
Set-ExecutionPolicy -Scope CurrentUser -ExecutionPolicy RemoteSigned
```

`RemoteSigned` is the safe default — it allows local scripts to run but
requires downloaded scripts to be signed.
