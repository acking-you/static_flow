#!/usr/bin/env pwsh
#
# One-click StaticFlow frontend build / serve / publish.
#
# Portable by design — no machine-specific paths. The script resolves the
# whole toolchain itself (wasm target, Trunk, the Tailwind CLI via npm) and
# then drives Trunk, so a fresh checkout builds with a single command. It is
# the local counterpart to the GitHub Pages CI in .github/workflows/deploy.yml
# and uses the same Tailwind hook (`npx @tailwindcss/cli`, declared in
# crates/frontend/Trunk.toml) so local and CI output match.
#
# Usage (from anywhere):
#   pwsh ./scripts/build_frontend.ps1                 # production bundle -> crates/frontend/dist
#   pwsh ./scripts/build_frontend.ps1 -Mode dev       # dev server (mock data) at http://localhost:8080
#   pwsh ./scripts/build_frontend.ps1 -Mode selfhosted# same-origin /api production bundle
#   pwsh ./scripts/build_frontend.ps1 -SkipNpm        # reuse already-installed node deps (faster)
#
# Windows PowerShell users can also run:
#   powershell -ExecutionPolicy Bypass -File scripts\build_frontend.ps1
#
# The default 'build' / 'selfhosted' output in crates/frontend/dist is deploy
# ready (the GitHub Pages 404.html SPA shim and any standalone/ pages are
# copied in, matching CI). Set $env:STATICFLOW_API_BASE before a 'build' run to
# point the bundle at your production API; 'selfhosted' forces it to /api.

[CmdletBinding()]
param(
    [ValidateSet('build', 'dev', 'selfhosted')]
    [string]$Mode = 'build',
    [switch]$SkipNpm
)

$ErrorActionPreference = 'Stop'

function Info($m) { Write-Host "[build-frontend] $m" -ForegroundColor Cyan }
function Warn($m) { Write-Host "[build-frontend] $m" -ForegroundColor Yellow }
function Die($m) { Write-Host "[build-frontend][ERROR] $m" -ForegroundColor Red; exit 1 }

# Repo root is the parent of this script's directory; everything else is derived.
$RepoRoot = Split-Path -Parent $PSScriptRoot
$FrontendDir = Join-Path $RepoRoot 'crates/frontend'
if (-not (Test-Path $FrontendDir)) { Die "frontend directory not found: $FrontendDir" }

# --- 1. Rust + wasm target -------------------------------------------------
if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) {
    Die 'rustup not found. Install Rust from https://rustup.rs and re-run.'
}
$installed = rustup target list --installed
if ($installed -notcontains 'wasm32-unknown-unknown') {
    Info 'Adding the wasm32-unknown-unknown target...'
    rustup target add wasm32-unknown-unknown
}

# --- 2. Trunk (the wasm bundler) -------------------------------------------
if (-not (Get-Command trunk -ErrorAction SilentlyContinue)) {
    Info 'Trunk not found - installing with cargo (this is a one-time step)...'
    cargo install --locked trunk
    if (-not (Get-Command trunk -ErrorAction SilentlyContinue)) {
        Die 'Trunk install finished but `trunk` is still not on PATH. Add ~/.cargo/bin to PATH.'
    }
}

# --- 3. Tailwind CLI via node deps -----------------------------------------
# Trunk's pre_build hook runs `npx @tailwindcss/cli`; a one-time `npm install`
# vendors it into node_modules so npx resolves locally (no per-build download).
if (-not $SkipNpm) {
    if (-not (Get-Command npm -ErrorAction SilentlyContinue)) {
        Die 'npm / Node.js not found. Install Node.js 20+ from https://nodejs.org (or pass -SkipNpm if the Tailwind CLI is already available).'
    }
    $tailwindCli = Join-Path $FrontendDir 'node_modules/@tailwindcss/cli'
    if (-not (Test-Path $tailwindCli)) {
        Info 'Installing frontend node deps (Tailwind CLI)...'
        Push-Location $FrontendDir
        try { npm install } finally { Pop-Location }
    }
    else {
        Info 'Tailwind CLI already installed - skipping npm install.'
    }
}

# --- 4. Build / serve ------------------------------------------------------
$env:TRUNK_SKIP_VERSION_CHECK = 'true'

function Copy-PublishExtras {
    # Mirror the CI deploy step so a locally built dist/ is deploy-ready.
    $dist = Join-Path $FrontendDir 'dist'
    $notFound = Join-Path $FrontendDir '404.html'
    if (Test-Path $notFound) {
        Copy-Item $notFound (Join-Path $dist '404.html') -Force
        Info 'Copied 404.html into dist/ (GitHub Pages SPA routing).'
    }
    $standalone = Join-Path $FrontendDir 'standalone'
    if ((Test-Path $standalone) -and (Get-ChildItem $standalone -ErrorAction SilentlyContinue)) {
        Copy-Item $standalone $dist -Recurse -Force
        Info 'Copied standalone/ pages into dist/.'
    }
}

Push-Location $FrontendDir
try {
    switch ($Mode) {
        'dev' {
            Info 'Starting dev server with mock data at http://localhost:8080 (Ctrl+C to stop)...'
            trunk serve --features mock
        }
        'selfhosted' {
            Info 'Building self-hosted (same-origin /api) production bundle...'
            $env:STATICFLOW_API_BASE = '/api'
            trunk build --release
            Copy-PublishExtras
            Info "Done. Deploy this folder or point FRONTEND_DIST_DIR at it: $(Join-Path $FrontendDir 'dist')"
        }
        default {
            Info 'Building production bundle...'
            if (-not $env:STATICFLOW_API_BASE) {
                Warn 'STATICFLOW_API_BASE is not set; the bundle will use its built-in default API base. Set $env:STATICFLOW_API_BASE first to override.'
            }
            trunk build --release
            Copy-PublishExtras
            Info "Done. Deploy-ready bundle: $(Join-Path $FrontendDir 'dist')"
        }
    }
}
finally { Pop-Location }
