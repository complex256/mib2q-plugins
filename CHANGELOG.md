# Changelog

## 1.2.2 — 2026-10-30

### Changed

- `carmqtt-status` 1.0.1 and `tailscale-status` 1.0.1: the status icons are redrawn at 30×30 px, the
  built-in bar's native size, instead of being upscaled from 20×20.
- Both packages now require SDK **0.7.0**, which adds the 30 px status-icon limit. mibpkg updates the SDK
  before the plugins.

### Packages

| Package | Version |
|---|---|
| `carmqtt` | 0.3.3 |
| `tailscale` | 1.102.9 |

The other packages keep their 1.2.1 versions and are carried forward.

## 1.2.1 — 2026-10-29

### Fixed

- The `carmqtt` and `tailscale` packages now include the generated `requires.toml`; their shell
  packaging scripts previously copied only the plugin jar. They now require SDK 0.6.3 like the Java
  packages.

### Packages

| Package | Version |
|---|---|
| `carmqtt` | 0.3.2 |
| `tailscale` | 1.102.8 |
| `wireguard` | 1.0.2 |

The other packages keep their 1.2.0 versions and are carried forward.

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
