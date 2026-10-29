# third_party - vendored modules with QNX 6.5 patches

Wired in through `replace` directives in `../go.mod`. Each tree is the pristine
upstream module plus the changes listed here; nothing else is modified.

## `wireguard-go/` - golang.zx2c4.com/wireguard @ v0.0.0-20260522210424-ecfc5a8d5446

| Change | Why |
|---|---|
| `ipc/uapi_qnx.go` (new) | `device/` needs the `IpcError*` codes; no ipc file matched `GOOS=qnx`. Values from QNX `<errno.h>`. There's no UAPI socket listener - mibwg configures the device in-process. |
| `tun/tun_qnx.go` (new) | CreateTUN over the shared qnxtun package, so tailscaled gets a kernel tun on QNX. IPv4 only (no TUNSIFHEAD ioctl in the Go port). |
| removed `tun/tun_windows.go`, `tun/alignment_windows_test.go`, `conn/bind_windows.go`, `conn/winrio/` | drops the `wintun` dependency from the module graph |

## `x-sys/` - golang.org/x/sys @ v0.32.0

| Change | Why |
|---|---|
| `unix/unix_qnx.go` (new) | x/sys has no qnx target. Re-exports the part of the ported std `syscall` package (qnx65-armv7-toolchain `go/src/syscall`) that wireguard-go's `conn` and `rwcancel` use. Add names as the compiler asks for them. |
| `unix/unix_qnx.go`: `Access`, `O_RDWR` | tailscale.com's `paths/paths_unix.go` (built from `mib2q-tailscale`, which shares this replace) calls `unix.Access(path, unix.O_RDWR)` to probe state-file writability. Both re-export straight from the ported std `syscall` package, same as everything else in this file. |
| `unix/unix_qnx.go`: `Dup`, `Dup2` | tailscale.com's `logtail/filch/filch_unix.go` calls `unix.Dup`/`unix.Dup2` to rotate its on-disk log-staging file. Both re-export straight from the ported std `syscall` package, same as everything else in this file. |
| `unix/unix_qnx.go`: `Getsid` | tailscale.com's `cmd/tailscale/cli/ssh_unix.go` references `unix.Getsid` to type-check a `runtime.GOOS == "linux"`-only code path; unreachable on qnx at runtime, but the symbol must still exist to compile. Re-exports straight from the ported std `syscall` package. |

gVisor (netstack) needs no patches: its tcpip packages have generic fallbacks
for unknown GOOS.

## Updating

```sh
go get golang.zx2c4.com/wireguard@<new>   # temporarily drop the replace first
cp -r $(go env GOMODCACHE)/golang.zx2c4.com/wireguard@<new> third_party/wireguard-go
chmod -R u+w third_party/wireguard-go
# re-apply the table above, then go mod tidy and re-run tests/ on QEMU
```
