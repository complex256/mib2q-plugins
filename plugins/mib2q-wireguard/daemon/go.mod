module github.com/complex256/mib2q-wireguard

go 1.24.1

require (
	golang.org/x/crypto v0.37.0
	golang.zx2c4.com/wireguard v0.0.0-20260522210424-ecfc5a8d5446
)

require (
	github.com/google/btree v1.1.2 // indirect
	golang.org/x/net v0.39.0 // indirect
	golang.org/x/sys v0.32.0 // indirect
	golang.org/x/time v0.7.0 // indirect
	gvisor.dev/gvisor v0.0.0-20250503011706-39ed1f5ac29c // indirect
)

replace golang.zx2c4.com/wireguard => ./third_party/wireguard-go

replace golang.org/x/sys => ./third_party/x-sys
