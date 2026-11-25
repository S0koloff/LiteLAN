param([Parameter(Mandatory=$true)][string]$SourceDir)
$ErrorActionPreference = 'Stop'
$file = Join-Path $SourceDir 'service/auth_status.go'
$text = [IO.File]::ReadAllText($file).Replace("`r`n", "`n")
function Replace-Exact([string]$Old,[string]$New) {
  if (!$script:text.Contains($Old) -or $script:text.IndexOf($Old) -ne $script:text.LastIndexOf($Old)) { throw 'AWL v0.21.0 source did not match the expected patch context.' }
  $script:text = $script:text.Replace($Old,$New)
}
# Keep the upstream stale incoming-request lifetime unchanged. Only speed up the outgoing retry ticker.
Replace-Exact 'ticker := time.NewTicker(backgroundRetryAuthRequests)' 'ticker := time.NewTicker(20 * time.Second)'
Replace-Exact 'ticker.Reset(backgroundRetryAuthRequests)' 'ticker.Reset(20 * time.Second)'
$old = @'
		// as with the status info, onPeerConnected sends it to a peer that is not connected now
		if !s.p2p.IsConnected(peerID) {
			continue
		}
		_ = s.SendAuthRequest(ctx, peerID, auth)
'@
$new = @'
		// LiteLAN: an initial dial can fail before a peer is discoverable.
		// Retry only pending, locally requested authorizations. ConnectPeer performs
		// the normal bounded discovery/dial; never bypass remote authorization.
		known, exists := s.conf.GetPeer(peerID.String())
		if !exists || known.Confirmed || known.Declined {
			continue
		}
		if _, blocked := s.conf.GetBlockedPeer(peerID.String()); blocked {
			continue
		}
		if err := s.SendAuthRequest(ctx, peerID, auth); err != nil {
			s.logger.Debugf("pending authorization retry to %s: %v", peerID, err)
		}
'@
Replace-Exact $old.Replace("`r`n","`n").TrimEnd("`n") $new.Replace("`r`n","`n").TrimEnd("`n")
$old2 = @'
		if sendAuth {
			_ = s.SendAuthRequest(ctx, peerID, authPeer)
		}
'@
$new2 = @'
		if sendAuth {
			if err := s.SendAuthRequest(ctx, peerID, authPeer); err != nil {
				s.logger.Debugf("initial authorization to %s: %v", peerID, err)
			}
		}
'@
Replace-Exact $old2.Replace("`r`n","`n").TrimEnd("`n") $new2.Replace("`r`n","`n").TrimEnd("`n")
[IO.File]::WriteAllText($file,$text,[Text.UTF8Encoding]::new($false))

# Local desktop actions. The same saved auth request is reused; never insert a duplicate peer.
$text = [IO.File]::ReadAllText($file)
$old = 'authsLock     sync.RWMutex'
$new = "authsLock     sync.RWMutex`n`t liteRetries sync.Map"
Replace-Exact $old $new
# Ensure status/IP exchange follows successful retry, even if the link was already open.
$old3 = @'
		if err := s.SendAuthRequest(ctx, peerID, auth); err != nil {
			s.logger.Debugf("pending authorization retry to %s: %v", peerID, err)
		}
'@
$new3 = @'
		_ = auth // current request is read again by LiteRetryPeer
		_ = s.LiteRetryPeer(ctx, peerID)
'@
Replace-Exact $old3.Replace("`r`n","`n").TrimEnd("`n") $new3.Replace("`r`n","`n").TrimEnd("`n")
$extra = @'

// LiteRetryPeer retries an existing request or refreshes an authorized peer.
// It never changes approval, creates a peer, or creates a new invite token.
func (s *AuthStatus) LiteRetryPeer(ctx context.Context, id peer.ID) error {
    if _, loaded := s.liteRetries.LoadOrStore(id, struct{}{}); loaded { return nil }
    defer s.liteRetries.Delete(id)
    known, ok := s.conf.GetPeer(id.String())
    if !ok || known.Declined { return ErrUnknownPeer }
    if _, blocked := s.conf.GetBlockedPeer(id.String()); blocked { return ErrUnknownPeer }
    ctx, cancel := context.WithTimeout(ctx, 30*time.Second)
    defer cancel()
    if !known.Confirmed {
        s.authsLock.RLock()
        auth, pending := s.outgoingAuths[id]
        s.authsLock.RUnlock()
        if !pending { return fmt.Errorf("no pending authorization") }
        if err := s.SendAuthRequest(ctx, id, auth); err != nil {
            s.logger.Debugf("LiteLAN auth retry to %s: %v", id, err)
            return err
        }
    }
    known, ok = s.conf.GetPeer(id.String())
    if !ok || known.Declined || !known.Confirmed { return nil }
    if err := s.p2p.ConnectPeer(ctx, id); err != nil { return err }
    return s.ExchangeNewStatusInfo(ctx, id, known)
}

// LiteDropPending clears local retries when the user cancels a pending invitation.
func (s *AuthStatus) LiteDropPending(id peer.ID) { s.dropOutgoingAuth(id) }
'@
$text += "`n" + $extra + "`n"
[IO.File]::WriteAllText($file,$text,[Text.UTF8Encoding]::new($false))
$router = Join-Path $SourceDir 'api/api.go'
$text = [IO.File]::ReadAllText($router).Replace("`r`n", "`n")
Replace-Exact 'e.POST(SendFriendRequestPath, h.SendFriendRequest)' "e.POST(SendFriendRequestPath, h.SendFriendRequest)`n`te.POST(V0Prefix+`"peers/litelan_retry`", h.LiteRetryPeer)`n`te.POST(V0Prefix+`"peers/litelan_cancel`", h.LiteCancelPeer)"
[IO.File]::WriteAllText($router,$text,[Text.UTF8Encoding]::new($false))
$handlers = Join-Path $SourceDir 'api/litelan.go'
$extra = @'
package api

import (
    "net/http"
    "github.com/labstack/echo/v4"
    "github.com/libp2p/go-libp2p/core/peer"
    "github.com/anywherelan/awl/entity"
)

func (h *Handler) LiteRetryPeer(c echo.Context) error {
    var req entity.PeerIDRequest
    if err := c.Bind(&req); err != nil { return c.JSON(http.StatusBadRequest, ErrorMessage("invalid request")) }
    id, err := peer.Decode(req.PeerID)
    if err != nil { return c.JSON(http.StatusBadRequest, ErrorMessage("invalid peer ID")) }
    p, ok := h.conf.GetPeer(req.PeerID)
    if !ok || p.Declined { return c.JSON(http.StatusConflict, ErrorMessage("device missing or declined")) }
    if _, blocked := h.conf.GetBlockedPeer(req.PeerID); blocked { return c.JSON(http.StatusConflict, ErrorMessage("device blocked")) }
    go func(){ if err := h.authStatus.LiteRetryPeer(h.ctx, id); err != nil { h.logger.Debugf("LiteLAN retry %s: %v", id, err) } }()
    return c.NoContent(http.StatusAccepted)
}

func (h *Handler) LiteCancelPeer(c echo.Context) error {
    var req entity.PeerIDRequest
    if err := c.Bind(&req); err != nil { return c.JSON(http.StatusBadRequest, ErrorMessage("invalid request")) }
    id, err := peer.Decode(req.PeerID)
    if err != nil { return c.JSON(http.StatusBadRequest, ErrorMessage("invalid peer ID")) }
    p, ok := h.conf.GetPeer(req.PeerID)
    if !ok || p.Confirmed || p.Declined { return c.JSON(http.StatusConflict, ErrorMessage("only a pending invitation can be cancelled")) }
    if _, ok := h.conf.RemovePeer(req.PeerID); !ok { return c.JSON(http.StatusNotFound, ErrorMessage("device missing")) }
    h.authStatus.LiteDropPending(id)
    h.p2p.UnprotectPeer(id)
    return c.NoContent(http.StatusOK)
}
'@
[IO.File]::WriteAllText($handlers,$extra,[Text.UTF8Encoding]::new($false))
