// Copyright 2026 mib2q-wireguard. BSD-style license, as the rest of x/sys.

//go:build qnx

// Package unix, QNX 6.5 port shim.
//
// x/sys has no qnx target. Rather than generating a full zsyscall/ztypes set,
// this file re-exports the subset of the ported std syscall package
// (qnx65-armv7-toolchain/go/src/syscall) that wireguard-go and gVisor's
// netstack actually reach for. Add names here as the compiler asks for them;
// every value comes from the QNX 6.5 sysroot via the std port.
package unix

import "syscall"

type (
	Errno    = syscall.Errno
	Signal   = syscall.Signal
	Timespec = syscall.Timespec
	Timeval  = syscall.Timeval
	PollFd   = syscall.PollFd
	Sockaddr = syscall.Sockaddr
)

const (
	EAGAIN      = syscall.EAGAIN
	EINTR       = syscall.EINTR
	EINVAL      = syscall.EINVAL
	EIO         = syscall.EIO
	EBADF       = syscall.EBADF
	ENOSYS      = syscall.ENOSYS
	EWOULDBLOCK = syscall.EWOULDBLOCK
	EADDRINUSE  = syscall.EADDRINUSE
	EPROTO      = Errno(71) // QNX errno.h; absent from the std port

	POLLIN   = syscall.POLLIN
	POLLOUT  = syscall.POLLOUT
	POLLERR  = syscall.POLLERR
	POLLHUP  = syscall.POLLHUP
	POLLNVAL = syscall.POLLNVAL

	AF_INET      = syscall.AF_INET
	AF_INET6     = syscall.AF_INET6
	AF_UNIX      = syscall.AF_UNIX
	SOCK_DGRAM   = syscall.SOCK_DGRAM
	SOCK_STREAM  = syscall.SOCK_STREAM
	SOL_SOCKET   = syscall.SOL_SOCKET
	SO_RCVBUF    = syscall.SO_RCVBUF
	SO_SNDBUF    = syscall.SO_SNDBUF
	SO_REUSEADDR = syscall.SO_REUSEADDR
	IPPROTO_IP   = syscall.IPPROTO_IP
	IPPROTO_IPV6 = syscall.IPPROTO_IPV6
	IPPROTO_TCP  = syscall.IPPROTO_TCP
	IPPROTO_UDP  = syscall.IPPROTO_UDP
	IPV6_V6ONLY  = syscall.IPV6_V6ONLY

	O_RDWR = syscall.O_RDWR
)

func Poll(fds []PollFd, timeout int) (int, error) { return syscall.Poll(fds, timeout) }
func Read(fd int, p []byte) (int, error)          { return syscall.Read(fd, p) }
func Write(fd int, p []byte) (int, error)         { return syscall.Write(fd, p) }
func Close(fd int) error                          { return syscall.Close(fd) }
func SetNonblock(fd int, nonblocking bool) error  { return syscall.SetNonblock(fd, nonblocking) }
func SetsockoptInt(fd, level, opt, value int) error {
	return syscall.SetsockoptInt(fd, level, opt, value)
}
func GetsockoptInt(fd, level, opt int) (int, error)  { return syscall.GetsockoptInt(fd, level, opt) }
func Getpid() int                                    { return syscall.Getpid() }
func Kill(pid int, sig Signal) error                 { return syscall.Kill(pid, sig) }
func ClockSettime(clockid int32, ts *Timespec) error { return syscall.ClockSettime(clockid, ts) }
func NsecToTimespec(nsec int64) Timespec             { return syscall.NsecToTimespec(nsec) }
func Socket(domain, typ, proto int) (int, error)     { return syscall.Socket(domain, typ, proto) }
func Bind(fd int, sa Sockaddr) error                 { return syscall.Bind(fd, sa) }
func Sendto(fd int, p []byte, flags int, to Sockaddr) error {
	return syscall.Sendto(fd, p, flags, to)
}
func Recvfrom(fd int, p []byte, flags int) (int, Sockaddr, error) {
	return syscall.Recvfrom(fd, p, flags)
}
func Access(path string, mode uint32) error { return syscall.Access(path, mode) }
func Dup(fd int) (int, error)               { return syscall.Dup(fd) }
func Dup2(old, new int) error               { return syscall.Dup2(old, new) }
func Getsid(pid int) (int, error)           { return syscall.Getsid(pid) }
