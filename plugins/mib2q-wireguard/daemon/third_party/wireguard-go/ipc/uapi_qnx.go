/* SPDX-License-Identifier: MIT
 *
 * Copyright (C) 2017-2025 WireGuard LLC. All Rights Reserved.
 */

//go:build qnx

// QNX 6.5 port (mib2q-wireguard): no UAPI socket listener - the device is
// configured in-process via IpcSet - but device/ still needs the error codes.

package ipc

import "syscall"

const (
	IpcErrorIO        = -int64(syscall.EIO)
	IpcErrorProtocol  = -71 // EPROTO (QNX errno.h; not in the port's syscall)
	IpcErrorInvalid   = -int64(syscall.EINVAL)
	IpcErrorPortInUse = -int64(syscall.EADDRINUSE)
	IpcErrorUnknown   = -55 // ENOANO
)
