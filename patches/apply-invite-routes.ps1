param([Parameter(Mandatory=$true)][string]$SourceDir)
$ErrorActionPreference = 'Stop'
function Patch([string]$Path,[string]$Old,[string]$New) {
  $file=Join-Path $SourceDir $Path
  $text=[IO.File]::ReadAllText($file).Replace("`r`n","`n")
  if (!$text.Contains($Old) -or $text.IndexOf($Old) -ne $text.LastIndexOf($Old)) { throw "AWL source mismatch: $Path" }
  [IO.File]::WriteAllText($file,$text.Replace($Old,$New),[Text.UTF8Encoding]::new($false))
}
Copy-Item (Join-Path $PSScriptRoot 'litelan_invite_routes.go') (Join-Path $SourceDir 'p2p/litelan_invite_routes.go') -Force
Copy-Item (Join-Path $PSScriptRoot 'litelan_invite_routes_test.go') (Join-Path $SourceDir 'p2p/litelan_invite_routes_test.go') -Force
Copy-Item (Join-Path $PSScriptRoot 'litelan_invite_api.go') (Join-Path $SourceDir 'api/litelan_invite_routes.go') -Force
Patch 'p2p/p2p.go' 'redials sync.Map' "redials sync.Map`n`t liteRelays liteRelayState"
$old = @'
	// FindPeer runs until peer is found in DHT or context is cancelled, so a timeout is mandatory
'@
$new = @'
	// LiteLAN: cached invite addresses can work when DHT cannot discover a peer.
	if len(p.host.Peerstore().Addrs(peerID)) > 0 {
		if err := p.liteDialCached(ctx, peerID); err == nil || p.IsConnected(peerID) { return nil }
	}
	// FindPeer runs until peer is found in DHT or context is cancelled, so a timeout is mandatory
'@
Patch 'p2p/p2p.go' $old.Replace("`r`n","`n").TrimEnd("`n") $new.Replace("`r`n","`n").TrimEnd("`n")
Patch 'api/api.go' 'e.POST(CreateInvitePath, h.CreateInvite)' "e.POST(CreateInvitePath, h.CreateInvite)`n`t e.POST(V0Prefix+`"peers/litelan_prepare_invite`", h.LitePrepareInvite)`n`t e.POST(V0Prefix+`"peers/litelan_invite_routes`", h.LiteSaveInviteRoutes)"
Patch 'api/api.go' 'h.echo = e1' "h.echo = e1`n`t go h.LiteMaintainInviteRelays()"
