param([Parameter(Mandatory=$true)][string]$OutputExe)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$cache = Join-Path $root '.cache'
$source = Join-Path $cache 'awl-source-v0.21.0'
if (Test-Path $source) { Remove-Item $source -Recurse -Force }
git clone --depth 1 --branch v0.21.0 https://github.com/anywherelan/awl.git $source
if ($LASTEXITCODE -ne 0) { throw 'AWL source checkout failed' }
& (Join-Path $PSScriptRoot 'apply-auth-retry.ps1') -SourceDir $source
# Upstream embeds its web assets; LiteLAN has its own native frontend.
New-Item -ItemType Directory -Force (Join-Path $source 'static') | Out-Null
if (!(Test-Path (Join-Path $source 'static/index.html'))) {
  Set-Content -Encoding ascii (Join-Path $source 'static/index.html') '<!doctype html><title>LiteLAN core</title><p>Use the LiteLAN desktop client.</p>'
}
$wintunZip = Join-Path $cache 'wintun-0.14.1.zip'
if (!(Test-Path $wintunZip)) { Invoke-WebRequest -UseBasicParsing 'https://www.wintun.net/builds/wintun-0.14.1.zip' -OutFile $wintunZip }
$wintunDir = Join-Path $cache 'wintun-extracted'
Expand-Archive $wintunZip $wintunDir -Force
Copy-Item (Join-Path $wintunDir 'wintun/bin/amd64/wintun.dll') (Join-Path $source 'embeds/wintun.dll') -Force
$env:CGO_ENABLED = '0'
$env:GOOS = 'windows'
$env:GOARCH = 'amd64'
Push-Location $source
try {
  gofmt -w service/auth_status.go api/api.go api/litelan.go
  if ($LASTEXITCODE -ne 0) { throw 'AWL patch formatting failed' }
  go build -trimpath -ldflags '-buildid= -s -w -H windowsgui -X github.com/anywherelan/awl/config.Version=v0.21.0' -o $OutputExe ./cmd/awl
  if ($LASTEXITCODE -ne 0) { throw 'Patched AWL build failed' }
} finally { Pop-Location }
