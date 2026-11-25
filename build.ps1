$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { throw 'Install Rust via https://rustup.rs/ and Visual Studio Build Tools (Desktop development with C++), then reopen terminal.' }
$asset = 'awl-windows-amd64-v0.21.0.zip'
$sha = 'e6d2160446281dd9650616d2c2427ba5bc0f0edab45a393a73f25b136d9368e4'
$cache = Join-Path $PSScriptRoot '.cache'
New-Item -ItemType Directory -Force $cache | Out-Null
$archive = Join-Path $cache $asset
if (!(Test-Path $archive)) {
  Invoke-WebRequest -UseBasicParsing "https://github.com/anywherelan/awl/releases/download/v0.21.0/$asset" -OutFile $archive
}
if ((Get-FileHash $archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne $sha) { throw "AWL archive checksum mismatch. Delete $archive and retry." }
$unpack = Join-Path $cache 'awl'
Expand-Archive $archive $unpack -Force
$exe = Get-ChildItem $unpack -Recurse -File -Filter 'awl.exe' | Select-Object -First 1
if (!$exe) { throw 'awl.exe not found in official archive' }
cargo test --target x86_64-pc-windows-msvc
if ($LASTEXITCODE -ne 0) { throw 'Tests failed' }
cargo build --release --target x86_64-pc-windows-msvc
if ($LASTEXITCODE -ne 0) { throw 'Build failed' }
$dist = Join-Path $PSScriptRoot 'dist'
if (Test-Path $dist) { Remove-Item $dist -Recurse -Force }
New-Item -ItemType Directory -Force "$dist/core" | Out-Null
Copy-Item "$($exe.Directory.FullName)\*" "$dist/core" -Recurse -Force
Copy-Item 'target/x86_64-pc-windows-msvc/release/litelan.exe' "$dist/LiteLAN.exe" -Force
Copy-Item README.md,THIRD_PARTY.md "$dist/" -Force
Compress-Archive -Path "$dist/*" -DestinationPath "$PSScriptRoot/LiteLAN-win-x64.zip" -Force
Write-Host "Built: $dist/LiteLAN.exe"
