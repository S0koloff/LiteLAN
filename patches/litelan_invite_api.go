package api

import (
	"context"
	"net/http"
	"time"

	"github.com/labstack/echo/v4"
	"github.com/libp2p/go-libp2p/core/peer"
)

func (h *Handler) LitePrepareInvite(c echo.Context) error {
	ctx, cancel := context.WithTimeout(c.Request().Context(), 15*time.Second)
	defer cancel()
	routes, err := h.p2p.LiteInviteRoutes(ctx)
	if err != nil {
		return c.JSON(http.StatusServiceUnavailable, ErrorMessage(err.Error()))
	}
	return c.JSON(http.StatusOK, map[string]any{"addresses": routes})
}
func (h *Handler) LiteSaveInviteRoutes(c echo.Context) error {
	var req struct {
		PeerID    string   `json:"peerID"`
		Addresses []string `json:"addresses"`
	}
	if err := c.Bind(&req); err != nil {
		return c.JSON(http.StatusBadRequest, ErrorMessage("invalid route request"))
	}
	id, err := peer.Decode(req.PeerID)
	if err != nil {
		return c.JSON(http.StatusBadRequest, ErrorMessage("invalid peer ID"))
	}
	if _, blocked := h.conf.GetBlockedPeer(req.PeerID); blocked {
		return c.JSON(http.StatusConflict, ErrorMessage("device blocked"))
	}
	if err = h.p2p.LiteSaveInviteRoutes(id, req.Addresses); err != nil {
		return c.JSON(http.StatusBadRequest, ErrorMessage(err.Error()))
	}
	return c.NoContent(http.StatusOK)
}

// Renew while at least one invitation is usable; also restore after core restart.
func (h *Handler) LiteMaintainInviteRelays() {
	ticker := time.NewTicker(30 * time.Second)
	defer ticker.Stop()
	for {
		select {
		case <-h.ctx.Done():
			return
		case <-ticker.C:
		}
		active := false
		for _, i := range h.conf.ListInvites() {
			if !i.Revoked && !i.IsExpired(time.Now()) && !i.IsUsedUp() {
				active = true
				break
			}
		}
		if !active {
			continue
		}
		ctx, cancel := context.WithTimeout(h.ctx, 15*time.Second)
		_, err := h.p2p.LiteInviteRoutes(ctx)
		cancel()
		if err != nil {
			h.logger.Debugf("LiteLAN invite relay refresh: %v", err)
		}
	}
}
