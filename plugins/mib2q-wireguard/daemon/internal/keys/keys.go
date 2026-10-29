// Package keys generates WireGuard keys, like `wg genkey` / `wg pubkey`.
package keys

import (
	"crypto/rand"

	"golang.org/x/crypto/curve25519"

	"github.com/complex256/mib2q-wireguard/internal/config"
)

func NewPrivate() (config.Key, error) {
	var k config.Key
	if _, err := rand.Read(k[:]); err != nil {
		return k, err
	}
	// Clamp, as wg genkey does.
	k[0] &= 248
	k[31] = (k[31] & 127) | 64
	return k, nil
}

func Public(priv config.Key) (config.Key, error) {
	var pub config.Key
	b, err := curve25519.X25519(priv[:], curve25519.Basepoint)
	copy(pub[:], b)
	return pub, err
}
