# Yocto Integration

> Full content added in Phase 4.

## Overview

`meta-telemetry` is a custom Yocto BSP layer that packages `telemetry-linux`
as a systemd service in a minimal embedded Linux image.

## Strategy

- **Development target:** `MACHINE = "qemuarm64"` — validate the full image in QEMU
  before touching real hardware.
- **Real hardware target:** `MACHINE = "raspberrypi4-64"` — swap one variable, rest is identical.
- **CI:** Yocto builds are **not** run in CI (too heavy for hosted runners).
  This document is the reproducible build reference.
- **Planned:** self-hosted runner building the full image on release tags.

## Host dependencies (WSL Debian 13 / trixie)

```bash
sudo apt install -y \
    gawk wget git diffstat unzip texinfo gcc build-essential chrpath socat cpio \
    python3 python3-pip python3-pexpect xz-utils debianutils iputils-ping \
    python3-git python3-jinja2 python3-subunit zstd lz4 file locales libacl1

# For cargo-bitbake (needs OpenSSL headers)
sudo apt install -y pkg-config libssl-dev
cargo install cargo-bitbake
```

## Layer layout

```
yocto/meta-telemetry/
  conf/layer.conf
  recipes-telemetry/
    telemetry-logger/
      telemetry-logger_0.1.0.bb    # cargo-bitbake generated + hand-tuned
      files/
        telemetry-logger.service   # systemd unit
        telemetry-logger.toml      # default config
  recipes-core/images/
    telemetry-image.bbappend       # adds service + binary to core-image-minimal
```

## Build steps

_To be written in Phase 4._

## QEMU boot

```bash
runqemu qemuarm64 nographic
```

Verify: `systemctl status telemetry-logger` → active (running).

## Real Pi 4/5

Change `MACHINE = "raspberrypi4-64"` and rebuild.  Flash with `dd` or `rpi-imager`.

## Planned targets

- NXP i.MX8 (`MACHINE = "imx8mmevk"`) via `meta-freescale`.
