# Gera o exe para publicar em dist\, sem caminhos desta máquina dentro dele.
# Uso: powershell -ExecutionPolicy Bypass -File tools\gerar-release.ps1
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
$cargo = Join-Path $cargoHome 'bin\cargo.exe'
if (-not (Test-Path $cargo)) { $cargo = 'cargo' }

# As mensagens de erro do Rust guardam o caminho dos arquivos de código; aqui eles viram nomes genéricos.
$env:RUSTFLAGS = "--remap-path-prefix=$cargoHome=/cargo --remap-path-prefix=$root=/ar-tray -C link-arg=/PDBALTPATH:%_PDB%"
$env:CARGO_TARGET_DIR = Join-Path $root 'target\dist'
# Cargo writes progress to stderr, which PowerShell 5 would treat as an error under 'Stop'.
$ErrorActionPreference = 'Continue'
& $cargo build --release --manifest-path (Join-Path $root 'Cargo.toml')
$ErrorActionPreference = 'Stop'
if ($LASTEXITCODE -ne 0) { throw 'cargo build falhou' }

$version = (Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
$out = Join-Path $root "dist\PainelDoAr-$version-windows-x64.exe"
New-Item -ItemType Directory -Force (Split-Path $out) | Out-Null
Copy-Item (Join-Path $env:CARGO_TARGET_DIR 'release\ar-tray.exe') $out -Force
$hash = (Get-FileHash $out -Algorithm SHA256).Hash.ToLower()
"$hash  $(Split-Path $out -Leaf)" | Set-Content -Encoding ascii "$out.sha256"
"Gerado: $out"
"SHA-256: $hash"
