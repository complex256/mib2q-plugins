module github.com/complex256/mib2q-tailscale

go 1.26.4

require (
	github.com/complex256/mib2q-wireguard v0.0.0
	golang.zx2c4.com/wireguard v0.0.0
)

replace github.com/complex256/mib2q-wireguard => ../../mib2q-wireguard/daemon

replace golang.zx2c4.com/wireguard => ../../mib2q-wireguard/daemon/third_party/wireguard-go

replace golang.org/x/sys => ../../mib2q-wireguard/daemon/third_party/x-sys
