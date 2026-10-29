//go:build !qnx

package backend

import "errors"

// The kernel-routed backends drive io-pkt interfaces, so they're QNX-only.

func NewGif(KernelOptions) (Backend, error) {
	return nil, errors.New("the gif backend only runs on QNX")
}

func NewTun(KernelOptions) (Backend, error) {
	return nil, errors.New("the tun backend only runs on QNX")
}
