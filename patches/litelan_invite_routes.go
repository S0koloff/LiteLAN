package p2p

import (
	"context"
	"fmt"
	"sync"
	"time"

	"github.com/libp2p/go-libp2p/core/network"
	"github.com/libp2p/go-libp2p/core/peer"
	"github.com/libp2p/go-libp2p/p2p/protocol/circuitv2/client"
	ma "github.com/multiformats/go-multiaddr"
	manet "github.com/multiformats/go-multiaddr/net"
)

// Keep explicit invitation reservations independent of AutoNAT/AutoRelay.
// The bootstrap peers already have protected connections in AWL.
type liteRelayLease struct {
	expires time.Time
	addrs   []ma.Multiaddr
}
type liteRelayState struct {
	mu     sync.Mutex
	leases map[peer.ID]liteRelayLease
}

func liteDirect(a ma.Multiaddr) bool {
	p := a.Protocols()
	if len(p) < 2 || (p[0].Code != ma.P_IP4 && p[0].Code != ma.P_IP6) || !manet.IsPublicAddr(a) {
		return false
	}
	return (len(p) == 2 && p[1].Code == ma.P_TCP) || (len(p) == 3 && p[1].Code == ma.P_UDP && p[2].Code == ma.P_QUIC_V1)
}
func liteIPv4(a ma.Multiaddr) bool { return liteDirect(a) && a.Protocols()[0].Code == ma.P_IP4 }
func liteRelayRoutes(info peer.AddrInfo) []ma.Multiaddr {
	suffix, err := ma.NewMultiaddr("/p2p/" + info.ID.String() + "/p2p-circuit")
	if err != nil {
		return nil
	}
	var out []ma.Multiaddr
	for _, a := range info.Addrs {
		if liteIPv4(a) {
			out = append(out, a.Encapsulate(suffix))
		}
	}
	return out
}

// Prepare before minting the invite. Only successfully reserved IPv4 relay
// routes are put in the link; do not infer a reservation from a bootstrap ping.
func (p *P2p) LiteInviteRoutes(ctx context.Context) ([]string, error) {
	st := &p.liteRelays
	st.mu.Lock()
	defer st.mu.Unlock()
	if st.leases == nil {
		st.leases = make(map[peer.ID]liteRelayLease)
	}
	type result struct {
		id    peer.ID
		lease liteRelayLease
		err   error
	}
	candidates := make([]peer.AddrInfo, 0, 4)
	var routes []string
	now := time.Now()
	// Prefer existing leases, keeping the same routes in already shared links.
	for _, info := range p.bootstrapPeers {
		if len(liteRelayRoutes(info)) == 0 {
			continue
		}
		lease, ok := st.leases[info.ID]
		if ok && p.IsConnected(info.ID) && lease.expires.After(now.Add(10*time.Minute)) {
			for _, a := range lease.addrs {
				routes = append(routes, a.String())
			}
			continue
		}
		if p.IsConnected(info.ID) {
			// Keep a still-valid route if its early renewal times out.
			if ok && lease.expires.After(now.Add(time.Minute)) {
				for _, a := range lease.addrs {
					routes = append(routes, a.String())
				}
			}
			candidates = append(candidates, info)
		}
	}
	// Run the small fixed bootstrap set in parallel, at most four candidates.
	// Refresh previously issued routes even if another lease remains healthy.
	ch := make(chan result, len(candidates))
	for _, info := range candidates {
		go func(info peer.AddrInfo) {
			c, cancel := context.WithTimeout(ctx, 8*time.Second)
			defer cancel()
			r, e := client.Reserve(c, p.host, info)
			if e != nil {
				ch <- result{id: info.ID, err: e}
				return
			}
			ch <- result{id: info.ID, lease: liteRelayLease{expires: r.Expiration, addrs: liteRelayRoutes(info)}}
		}(info)
	}
collect:
	for range candidates {
		select {
		case r := <-ch:
			if r.err != nil {
				p.logger.Debugf("LiteLAN invite relay reservation %s failed: %v", r.id, r.err)
				continue
			}
			st.leases[r.id] = r.lease
			p.logger.Infof("LiteLAN invite IPv4 relay reserved: %s until %s", r.id, r.lease.expires.Format(time.RFC3339))
			for _, a := range r.lease.addrs {
				routes = append(routes, a.String())
			}
		case <-ctx.Done():
			break collect
		}
	}
	// Deduplicate when a still-valid lease was renewed.
	seen := map[string]bool{}
	unique := make([]string, 0, len(routes))
	for _, r := range routes {
		if !seen[r] {
			unique = append(unique, r)
			seen[r] = true
		}
	}
	routes = unique
	if len(routes) == 0 {
		return nil, fmt.Errorf("no IPv4 relay reservation available; keep LiteLAN online and try again")
	}
	// Announced public addresses are candidates, not proof of reachability.
	for _, a := range p.host.Addrs() {
		if liteDirect(a) && len(routes) < 16 {
			routes = append(routes, a.String())
		}
	}
	return routes, nil
}

// Address hints never authorize a peer. Limit them to public TCP/QUIC or the
// exact IPv4 relay routes of configured AWL bootstrap nodes.
func (p *P2p) LiteSaveInviteRoutes(id peer.ID, raw []string) error {
	if id == p.host.ID() || len(raw) > 16 {
		return fmt.Errorf("invalid invite route list")
	}
	allowed := map[string]bool{}
	for _, i := range p.bootstrapPeers {
		for _, a := range liteRelayRoutes(i) {
			allowed[a.String()] = true
		}
	}
	addrs := make([]ma.Multiaddr, 0, len(raw))
	seen := map[string]bool{}
	for _, s := range raw {
		if len(s) > 512 {
			return fmt.Errorf("invite route too long")
		}
		a, e := ma.NewMultiaddr(s)
		if e != nil {
			return fmt.Errorf("invalid invite route")
		}
		if !liteDirect(a) && !allowed[a.String()] {
			return fmt.Errorf("unsupported invite route")
		}
		if !seen[a.String()] {
			addrs = append(addrs, a)
			seen[a.String()] = true
		}
	}
	if len(addrs) > 0 {
		p.host.Peerstore().AddAddrs(id, addrs, 24*time.Hour)
		p.logger.Infof("LiteLAN stored %d invite routes for %s", len(addrs), id)
	}
	return nil
}

// Try cached hints without the routed host's mandatory lookup. Existing AWL
// authentication and hole punching remain unchanged. DHT is still a fallback.
func (p *P2p) liteDialCached(ctx context.Context, id peer.ID) error {
	addrs := p.host.Peerstore().Addrs(id)
	if len(addrs) == 0 {
		return fmt.Errorf("no cached addresses")
	}
	c, cancel := context.WithTimeout(ctx, 8*time.Second)
	defer cancel()
	c = network.WithAllowLimitedConn(c, "litelan-invite")
	err := p.basicHost.Connect(c, peer.AddrInfo{ID: id, Addrs: addrs})
	if err == nil {
		p.logger.Debugf("LiteLAN cached route connected to %s", id)
	} else {
		p.logger.Debugf("LiteLAN cached route failed for %s: %v", id, err)
	}
	return err
}
