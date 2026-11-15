# Builds the web version into web\dist: the folder to put on a website.
#
#   powershell -ExecutionPolicy Bypass -File web\build.ps1             (release, all three builds)
#   powershell -ExecutionPolicy Bypass -File web\build.ps1 -Quick      (release, only the build that runs everywhere)
#   powershell -ExecutionPolicy Bypass -File web\build.ps1 -NoThreads  (release, all but the many-threads build)
#   powershell -ExecutionPolicy Bypass -File web\build.ps1 -Dev        (quicker to build, bigger and slower)
#
# It makes three builds of the app and puts them side by side; a visitor
# downloads only one (index.html picks it):
#
#   golem_web_mt.js, golem_web_mt_bg.wasm, snippets\
#       many threads, draws with WebGPU. For a page that is cross-origin
#       isolated (the server sends the two headers in README.md) in a
#       browser with WebGPU.
#   golem_web_gpu.js, golem_web_gpu_bg.wasm
#       one thread, draws with WebGPU. For a browser with WebGPU on a page
#       that is not isolated.
#   golem_web.js, golem_web_bg.wasm
#       one thread, draws with WebGPU or WebGL2. For every other browser,
#       and the one the others fall back to. It is a fifth bigger: WebGL2
#       needs wgpu's own WebGPU implementation and shader translator.
#
# -Quick builds only golem_web (it runs everywhere, so it is enough for
# trying a change) and removes the other two from the folder, so the page
# cannot load a stale copy of them; -NoThreads does the same for the
# many-threads build alone, which takes the longest (it rebuilds Rust's
# standard library).
#
# Several people (or Claude agents) can build at once without stepping on
# each other: -Name gives a build its own target folders
# (target\w\web-<name>, -gpu and -mt) and its own output folder
# (web\dist-<name>), served with `python web\dist-<name>\serve.py <port>`.
#
# Needs, once:
#   rustup target add wasm32-unknown-unknown
#   cargo install wasm-bindgen-cli --version 0.2.129
# and for the many-threads build:
#   rustup toolchain install nightly --component rust-src --target wasm32-unknown-unknown
param([switch]$Dev, [string]$Name = '', [switch]$NoThreads, [switch]$Quick)
$ErrorActionPreference = 'Stop'
$web = $PSScriptRoot
$suffix = if ($Name) { "-$Name" } else { '' }
$target = Join-Path $web "..\target\w\web$suffix"
$profile = if ($Dev) { 'dev' } else { 'release' }
$folder = if ($Dev) { 'debug' } else { 'release' }
$dist = Join-Path $web "dist$suffix"
# The release build leaves out the function names (4 MB) only a debugger reads.
$strip = if ($Dev) { @() } else { @('--remove-name-section', '--remove-producers-section') }

# One build: cargo into its own target folder, then wasm-bindgen into the
# output folder as <name>.js and <name>_bg.wasm.
function Build-One([string]$OutName, [string]$TargetDir, [string[]]$CargoArgs) {
    cargo @CargoArgs --lib --target wasm32-unknown-unknown --profile $profile --target-dir $TargetDir
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($OutName)" }
    $wasm = Join-Path $TargetDir "wasm32-unknown-unknown\$folder\golem_web.wasm"
    wasm-bindgen $wasm --out-dir $dist --out-name $OutName --target web --no-typescript @strip
    if ($LASTEXITCODE -ne 0) { throw "wasm-bindgen failed ($OutName)" }
}

function Remove-Build([string]$OutName) {
    foreach ($f in @("$OutName.js", "${OutName}_bg.wasm")) {
        Remove-Item (Join-Path $dist $f) -ErrorAction SilentlyContinue
    }
}

Push-Location $web
try {
    New-Item -ItemType Directory -Force $dist | Out-Null

    # Runs everywhere: WebGPU or WebGL2, one thread, the ordinary (stable) compiler.
    Build-One 'golem_web' $target @('build')

    if ($Quick) {
        Remove-Build 'golem_web_gpu'
    } else {
        # WebGPU only, one thread.
        Build-One 'golem_web_gpu' "$target-gpu" @('build', '--no-default-features')
    }

    if ($Quick -or $NoThreads) {
        Remove-Build 'golem_web_mt'
    } else {
        # Many threads, WebGPU only. WebAssembly threads need the module's
        # memory shared between the page and its workers ("atomics"), and
        # Rust's standard library rebuilt to match, which only the nightly
        # compiler does (-Z build-std). The linker is told to make the memory
        # shared and handed in by the page (so each worker gets the same
        # one), to let it grow to 4 GB as the one-thread builds' can (a
        # shared memory must say its largest size up front), and to export
        # what wasm-bindgen needs to give each thread its own thread-locals.
        $oldFlags = $env:RUSTFLAGS
        $env:RUSTFLAGS = @(
            '-C target-feature=+atomics,+bulk-memory,+mutable-globals',
            '-C link-arg=--shared-memory', '-C link-arg=--import-memory', '-C link-arg=--max-memory=4294967296',
            '-C link-arg=--export=__wasm_init_tls', '-C link-arg=--export=__tls_size',
            '-C link-arg=--export=__tls_align', '-C link-arg=--export=__tls_base'
        ) -join ' '
        try {
            Build-One 'golem_web_mt' "$target-mt" @('+nightly', 'build', '--no-default-features', '--features', 'threads', '-Z', 'build-std=std,panic_abort')
        } finally {
            $env:RUSTFLAGS = $oldFlags
        }
        # wasm-bindgen-rayon's worker script starts each worker with the old
        # form of wasm-bindgen's start call, which works but prints a warning
        # per worker in the browser's console; this gives it the new form.
        Get-ChildItem (Join-Path $dist 'snippets') -Recurse -Filter 'workerHelpers*.js' | ForEach-Object {
            $js = [IO.File]::ReadAllText($_.FullName)
            $js = $js.Replace('pkg.default(data.module, data.memory)', 'pkg.default({ module_or_path: data.module, memory: data.memory })')
            [IO.File]::WriteAllText($_.FullName, $js, (New-Object Text.UTF8Encoding $false))
        }
    }

    # index.html, with each build's size written in so its progress bar is
    # right when the server compresses the download (0: that build is not here).
    $sizeOf = { param($f) $p = Join-Path $dist $f; if (Test-Path $p) { (Get-Item $p).Length } else { 0 } }
    $full = & $sizeOf 'golem_web_bg.wasm'
    $gpu = & $sizeOf 'golem_web_gpu_bg.wasm'
    $mt = & $sizeOf 'golem_web_mt_bg.wasm'
    $page = [IO.File]::ReadAllText((Join-Path $web 'index.html'))
    $marker = 'const SIZES = { mt: 0, gpu: 0, full: 0 }; // GOLEM_SIZES'
    if (-not $page.Contains($marker)) { throw 'index.html has lost its GOLEM_SIZES line' }
    $page = $page.Replace($marker, "const SIZES = { mt: $mt, gpu: $gpu, full: $full }; // GOLEM_SIZES")
    [IO.File]::WriteAllText((Join-Path $dist 'index.html'), $page, (New-Object Text.UTF8Encoding $false))
    Copy-Item (Join-Path $web 'serve.py') $dist -Force

    Write-Host ("Built {0} (MB of WebAssembly):" -f $dist)
    Write-Host ("  golem_web      {0,5:N1}  WebGPU or WebGL2, one thread" -f ($full / 1MB))
    if ($gpu) { Write-Host ("  golem_web_gpu  {0,5:N1}  WebGPU, one thread" -f ($gpu / 1MB)) }
    if ($mt) { Write-Host ("  golem_web_mt   {0,5:N1}  WebGPU, many threads" -f ($mt / 1MB)) }
} finally {
    Pop-Location
}
