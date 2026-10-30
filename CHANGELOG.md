# Changelog

## 1.2.0 — 2026-10-29

### Changed

- Rebuilt every package against SDK 0.6.3. Plugin packages now carry `requires.toml`, naming
  0.6.3 as the oldest SDK they run on; mibpkg 0.11.1 enforces it on install and update.
- Bumped all product packages so the new sidecar is published under a new version.

### Packages

| Package | Version |
|---|---|
| `bike-rack` | 1.1.2 |
| `hello-menu` | 1.4.2 |
| `inputs-demo` | 1.0.2 |
| `tailscale` | 1.102.7 |
| `carmqtt` | 0.3.1 |
| `wireguard` | 1.0.1 |

## 1.1.0 — 2026-10-29

### Changed

- One mibpkg package per product. The `tailscale` and `carmqtt` packages now
  include their status plugins, so installing the product package installs both
  the daemon and its UI.
- Rebuilt the Java plugins against SDK 0.6.2 and added the official feed
  publisher workflow.

### Packages

| Package | Version |
|---|---|
| `bike-rack` | 1.1.1 |
| `hello-menu` | 1.4.1 |
| `inputs-demo` | 1.0.1 |
| `tailscale` | 1.102.6 |
| `carmqtt` | 0.3.0 |
| `wireguard` | 1.0.0 |

## 1.0.0 — 2026-10-29

Initial monorepo release: one repository for the Java plugins and the native
Tailscale, carmqtt and WireGuard daemons.
