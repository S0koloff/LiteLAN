param([switch]$OriginalCore)
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
# Stage only the runtime and upstream legal notices, not the whole archive.
Copy-Item -LiteralPath $exe.FullName -Destination (Join-Path $dist 'core/awl.exe') -Force
Get-ChildItem -LiteralPath $exe.Directory.FullName -File | Where-Object {
  $_.Extension -ieq '.dll' -or $_.Name -match '^(LICENSE|LICENCE|COPYING|NOTICE|COPYRIGHT|THIRD[_ -]?PARTY)([._ -].*)?$'
} | ForEach-Object {
  Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $dist 'core') -Force
}
if (!$OriginalCore) {
  & (Join-Path $PSScriptRoot 'patches/build-core.ps1') -OutputExe (Join-Path $dist 'core/awl.exe')
  Copy-Item (Join-Path $cache 'awl-source-v0.21.0/LICENSE') (Join-Path $dist 'core/AWL-LICENSE.txt') -Force
  Copy-Item (Join-Path $PSScriptRoot 'patches/apply-auth-retry.ps1') (Join-Path $dist 'core/apply-auth-retry.ps1') -Force
  Copy-Item (Join-Path $PSScriptRoot 'patches/apply-invite-routes.ps1') (Join-Path $dist 'core/apply-invite-routes.ps1') -Force
  Copy-Item (Join-Path $PSScriptRoot 'patches/litelan_invite_*.go') (Join-Path $dist 'core') -Force
}
Copy-Item 'target/x86_64-pc-windows-msvc/release/litelan.exe' "$dist/LiteLAN.exe" -Force
# README.md and assets/ belong to the repository, not the binary release.
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'THIRD_PARTY.md') -Destination $dist -Force
Compress-Archive -Path "$dist/*" -DestinationPath "$PSScriptRoot/LiteLAN-win-x64.zip" -Force
Write-Host "Built: $dist/LiteLAN.exe"
