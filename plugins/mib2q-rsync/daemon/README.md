# rsyncd

`rsyncd` keeps directories synchronized on an MIB2 High head unit (MHI2Q,
QNX 6.5) by running `rsync` on a schedule, over Tailscale or any other network
path the unit can reach. It is a small Rust supervisor:

- resolves removable-media paths (`@media:<marker>/...`) against a list of
  mount points,
- gates runs on the tailnet and (optionally) the current Wi-Fi SSID,
- runs `rsync` with deterministic arguments and parses its itemized output,
- runs user lifecycle hooks with a `RSYNC_*` environment and a timeout,
- serves a loopback `key=value` status/control endpoint for the HMI plugin.

The package ships its own cross-compiled `rsync` binary, so the unit does not
need one installed.

## Build and test

```sh
cargo test                  # host; unit tests + fake-rsync end-to-end tests
scripts/build.sh            # cross-build in qnx65-armv7-toolchain:8.5-rust
scripts/package.sh          # build/bin/mpk/rsyncd-<version>.mpk (needs mibpkg)
```

`scripts/build.sh` builds the Rust daemon with the image's `build-std` helper
and cross-compiles upstream rsync 3.2.7 with `arm-unknown-nto-qnx6.5.0eabi-gcc`
(`scripts/build-rsync.sh`). The built `rsync` is cached at
`target/armv7-unknown-nto-qnx650/release/rsync`, so later daemon rebuilds are
fast.

## Command line

```
usage: rsyncd [--config PATH] [--rsync PATH] [--self-test] [--version]
```

| Option | Default | Meaning |
|---|---|---|
| `--config PATH` | `/mnt/app/root/rsyncd/rsyncd.toml` | TOML configuration file |
| `--rsync PATH` | `[daemon].rsync_bin`, else `/mnt/app/root/rsyncd/bin/rsync` | `rsync` executable; an explicit CLI value wins over the config |
| `--self-test` | | Run on-target smoke tests, print `SELFTEST OK`, exit 0 |
| `--version` | | Print `rsyncd 0.1.0`, exit 0 |

The packaged service invokes:

```
rsyncd --config @config/rsyncd.toml --rsync @pkg/bin/rsync
```

## Configuration

The file format is shown in full in `deploy/rsyncd.toml.example`.

```toml
[daemon]
status_port = 19852
rsync_bin = "rsync"
media_roots = ["/fs/sda0", "/fs/sdb0", "/fs/sd0", "/fs/sd1", "/fs/usb0"]
tailscale_status_addr = "127.0.0.1:19850"
mode = "auto"                # auto | wifi_allowlist | manual
allowed_ssids = []
poll_interval_s = 10

[log]
level = "info"               # error | warn | info | debug | trace
file = "/tmp/rsyncd.log"     # "-" = stderr
max_bytes = 1048576

[[jobs]]
id = "music"
name = "Music Library"
enabled = true
direction = "pull"           # pull = remote -> local, push = local -> remote
remote = "rsync://100.64.0.10/music/"
local = "@media:.music-sd/Music/"
interval_s = 1800
require_tailnet = true
fat_safe = true
delete = true
bwlimit_kbps = 4000
exclude = [".music-sd", ".rsync-*", "mibpkg/", "System Volume Information/"]
extra_args = []
# password_file = "/mnt/app/root/mibpkg/state/config/rsyncd/music.secret"

[jobs.hooks]
# pre_sync = "..."
on_change = "/mnt/app/root/scripts/refresh-hmi-media.sh"
# post_sync = "..."
# on_error = "..."
timeout_s = 30
```

### Validation

Configuration errors stop the daemon at startup with a message:

- job `id` must be non-empty and match `[A-Za-z0-9_-]+`, and be unique,
- `direction` must be `pull` or `push`,
- `remote` and `local` must be non-empty,
- `daemon.mode` must be `auto`, `wifi_allowlist` or `manual`,
- `log.level` must be a known level.

A blank job `name` falls back to the id. Unknown keys are rejected so a typo
does not silently disable a setting.

### Scheduling and modes

Each enabled job runs immediately at startup and then every `interval_s`.
Jobs run one at a time (one `rsync` at a time). A job that cannot run is
retried every `poll_interval_s`:

- `waiting_media` — the `@media:` marker is not present in any media root,
- `waiting_tailnet` — `require_tailnet = true` and the tailnet probe failed,
- `waiting_wifi` — `wifi_allowlist` mode and the current SSID is not allowed.

Modes:

| Mode | Periodic behavior |
|---|---|
| `auto` | run on schedule, subject to `require_tailnet` |
| `wifi_allowlist` | run only while the current SSID is in `allowed_ssids` (empty SSID never matches) |
| `manual` | never run on schedule; only an explicit `sync` command starts a job |

The tailnet probe connects to `tailscale_status_addr` and, if the endpoint
returns a `state=` line (the `tsstatus` sidecar does), requires it to be
`running`. An empty address disables the probe (so a `require_tailnet` job
waits forever). Explicit `sync` commands bypass the mode/SSID gate but still
require media and, when configured, the tailnet.

## Media path resolution

`local` is either an ordinary path or a marker expression:

```toml
local = "/fs/sda0/Music/"               # ordinary path
local = "@media:.music-sd/Music/"       # marker + relative path
local = "@media:.music-sd"              # marker itself
```

For `@media:<marker>/<relpath>` the daemon checks each root in
`daemon.media_roots` in order. The first root where `<root>/<marker>` exists
(file or directory) wins, and the job uses `<root>/<relpath>` as its local
path. The winning root is reported as `mount` and its path as
`RSYNC_MOUNT_ROOT`.

For an ordinary path that starts with a configured media root, that root is the
mount root and must itself be a directory. Any other ordinary path resolves
when the path or its parent exists.

A marker that is present but whose `<relpath>` does not exist is not an error:
`rsync` creates the destination directory (the daemon also creates it before
starting).

## rsync arguments

Always:

```
--itemize-changes --out-format=ITEM:%i|%l|%n
```

With `fat_safe = true` (the default; appropriate for the unit's FAT/exFAT
volumes):

```
-rt --modify-window=2 --no-perms --no-owner --no-group --no-links
--partial --partial-dir=.rsync-partial --temp-dir=.rsync-tmp
```

Otherwise: `-a --partial`.

Then, when configured: `--delete-after`, `--bwlimit=<kbps>`,
`--password-file=<path>`, one `--exclude=<pattern>` per entry, and finally
`extra_args`. Source and destination are appended last; `pull` puts the remote
first, `push` puts local first. The destination directory (and
`.rsync-partial` / `.rsync-tmp` for a FAT-safe pull) is created before `rsync`
starts.

After a successful run that changed files, the daemon runs `sync` best-effort
to flush buffers before the unit can be powered off.

### Itemized output

Stdout lines beginning with `ITEM:` are parsed as
`ITEM:<itemize>|<length>|<name>`:

| Itemize prefix | Counter |
|---|---|
| `*deleting` | `files_deleted` |
| `>f+++++++++`, `<f+++++++++`, `c` | `files_added` |
| `>f`, `<f` (anything else) | `files_updated` |

`<length>` is added to `bytes_xfer` for added/updated files, and `<name>`
becomes `current_file`.

## Lifecycle hooks

Hooks are shell commands run as `sh -c "<cmd>"`. They inherit the daemon's
environment plus:

| Variable | Value |
|---|---|
| `RSYNC_JOB_ID` | job id |
| `RSYNC_JOB_NAME` | job display name |
| `RSYNC_DIRECTION` | `pull` or `push` |
| `RSYNC_LOCAL_PATH` | resolved local path |
| `RSYNC_MOUNT_ROOT` | media root that was matched |
| `RSYNC_REMOTE` | configured remote |
| `RSYNC_FILES_ADDED` | itemized added count |
| `RSYNC_FILES_UPDATED` | itemized updated count |
| `RSYNC_FILES_DELETED` | itemized deleted count |
| `RSYNC_FILES_CHANGED` | added + updated + deleted |
| `RSYNC_BYTES_XFER` | bytes reported by itemized output |
| `RSYNC_DURATION_MS` | rsync duration in milliseconds |
| `RSYNC_HOOK_EVENT` | `pre_sync`, `on_change`, `post_sync` or `on_error` |
| `RSYNC_ERROR` | failure text for `on_error`; empty otherwise |

Ordering and failure behavior:

1. `pre_sync` — before `rsync`. Non-zero exit or timeout aborts the run and
   triggers `on_error`.
2. `rsync` — a non-zero exit (or spawn failure) triggers `on_error`.
3. `on_change` — only when `files_added + files_updated + files_deleted > 0`;
   a failure triggers `on_error` and stops before `post_sync`.
4. `post_sync` — after a successful `rsync` (and `on_change`), even when no
   files changed; a failure triggers `on_error`.
5. `on_error` — when any of the above fails. Its own failure is logged and does
   not replace the original error.

Each hook is killed after `timeout_s` (0 disables the timeout). Hook stdout is
discarded; stderr is captured for the log/status error text.

`last_hook_rc` in the status snapshot is the exit code of the most recent hook
that ran (0 on success, -1 for a timeout/spawn failure). A successful
`on_error` therefore resets it to 0; the underlying failure remains visible in
`last_err`.

## Status and control endpoint

The daemon listens on `127.0.0.1:<daemon.status_port>` (default `19852`). A
bind failure is logged and does not stop syncing.

A client may send nothing, or one command line, within 40 ms. The daemon then
writes a snapshot and closes (after a bounded 150 ms drain):

| Command | Effect |
|---|---|
| *(empty)* or `status` | no state change, just the snapshot |
| `sync` | schedule all enabled jobs immediately |
| `sync <job_id>` | schedule one enabled job immediately |
| `cancel` | kill the active `rsync`; the run ends as cancelled, not an error |
| `mode <auto\|wifi_allowlist\|manual>` | change the runtime mode (invalid modes are ignored) |
| `ssid <ssid>` | set the current Wi-Fi SSID; `ssid` with no value clears it |

Examples:

```sh
nc 127.0.0.1 19852                 # snapshot
printf 'sync\n' | nc 127.0.0.1 19852
printf 'sync music\n' | nc 127.0.0.1 19852
printf 'cancel\n' | nc 127.0.0.1 19852
```

Snapshot format:

```
v=1
daemon=rsyncd
version=0.1.0
state=idle                 # syncing | error | idle
mode=auto
tailnet=1
ssid=
active_job=
jobs=1
job.0.id=music
job.0.name=Music Library
job.0.state=idle           # idle | disabled | syncing | waiting_media | waiting_tailnet | waiting_wifi | error
job.0.direction=pull
job.0.media_ok=1
job.0.mount=/fs/sda0
job.0.current_file=
job.0.files_changed=0
job.0.bytes_xfer=0
job.0.last_ok_age_s=-1     # -1 until the first success; monotonic seconds
job.0.last_changed=0
job.0.last_hook_rc=0
job.0.last_err=
uptime_s=0
```

Top-level `state` is `syncing` while any job is running `rsync` or a hook,
otherwise `error` if any enabled job has a non-empty `last_err`, otherwise
`idle`. The unit's wall clock starts at 1970, so ages are monotonic seconds,
never timestamps.

## Package layout

The mibpkg package (`packaging/manifest.toml.in`) installs:

```
bin/rsyncd
bin/rsync
rsyncd.toml.example
```

and starts `rsyncd` with `restart = "always"` after the `tailnet` unit (or
after 5 minutes at the latest). The HMI plugin artifact is `rsync-status.jar`
with icons `rsync-on` / `rsync-off`.

Keep `password_file` and any credentials only on the unit (in the mibpkg config
directory), never in this repository, a commit or a log.
