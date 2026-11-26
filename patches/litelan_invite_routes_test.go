package p2p

import (
	"github.com/libp2p/go-libp2p/core/peer"
	ma "github.com/multiformats/go-multiaddr"
	"testing"
)

func TestLiteInvitePublicRoutes(t *testing.T) {
	for _, tc := range []struct {
		addr string
		want bool
	}{
		{"/ip4/8.8.8.8/tcp/4363", true},
		{"/ip4/8.8.8.8/udp/4363/quic-v1", true},
		{"/ip6/2001:4860::1/tcp/4363", true},
		{"/ip4/127.0.0.1/tcp/4363", false},
		{"/ip4/192.168.1.5/tcp/4363", false},
		{"/ip4/8.8.8.8/udp/4363", false},
		{"/ip6/::1/tcp/4363", false},
	} {
		a, e := ma.NewMultiaddr(tc.addr)
		if e != nil {
			t.Fatal(e)
		}
		if liteDirect(a) != tc.want {
			t.Errorf("%s: want %v", tc.addr, tc.want)
		}
	}
}
func TestLiteInviteRelayRoutes(t *testing.T) {
	id, e := peer.Decode("12D3KooWGRjpNYgFssihdgTDnr5rdhdh9ruMTbeT41h1fXfGmatZ")
	if e != nil {
		t.Fatal(e)
	}
	a, _ := ma.NewMultiaddr("/ip4/8.8.8.8/udp/6150/quic-v1")
	b, _ := ma.NewMultiaddr("/ip6/2001:4860::1/tcp/7250")
	got := liteRelayRoutes(peer.AddrInfo{ID: id, Addrs: []ma.Multiaddr{a, b}})
	if len(got) != 1 || got[0].String() != a.String()+"/p2p/"+id.String()+"/p2p-circuit" {
		t.Fatalf("unexpected relay routes %v", got)
	}
}
