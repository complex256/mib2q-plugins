//go:build qnx

/* SPDX-License-Identifier: MIT
 *
 * Copyright (C) 2017-2023 WireGuard LLC. All Rights Reserved.
 */

// This mirrors uapi_fake.go (built for wasm/plan9/aix/solaris/illumos):
// device/uapi.go only ever references these IpcError* constants, never
// UAPIListen/UAPIOpen - confirmed by grepping tailscale.com and this
// module's own device package. Nothing on qnx opens a UAPI socket, so a real
// listener would be unused code, not a missing feature.
package ipc

// Made up sentinel error codes, matching uapi_fake.go's.
const (
	IpcErrorIO        = 1
	IpcErrorInvalid   = 2
	IpcErrorPortInUse = 3
	IpcErrorUnknown   = 4
	IpcErrorProtocol  = 5
)
