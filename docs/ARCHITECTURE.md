# OpenLogi Architecture

OpenLogi is a six-crate Cargo workspace — a native, local-first alternative to
Logitech Options+ that controls Logitech mice over HID++. This document traces
the crate layering and the HID++ flow end to end. For the quick orientation an
AI agent needs before touching code, start with [`../CLAUDE.md`](../CLAUDE.md);
for the developer workflow see [`DEVELOPMENT.md`](DEVELOPMENT.md).

## 1. Overview

```
  openlogi-cli   -> openlogi-core, openlogi-hid, openlogi-assets
  openlogi-gui   -> openlogi-core, openlogi-hid, openlogi-hook, openlogi-assets
  openlogi-hid   -> openlogi-core
  openlogi-hook  -> openlogi-core
  openlogi-core     (foundation: no internal deps)
  openlogi-assets   (foundation: no internal deps)
```

Read `->` as "depends on".

`openlogi-core` is the foundation: types, TOML config, paths, and the
button/action catalog. It is deliberately I/O-free (except reading and writing
its own config file) and never depends on `hidpp`, `async-hid`, or any platform
API. To keep that boundary, core **mirrors** the few HID++ types it needs (for
example `DeviceKind`, which mirrors `hidpp::receiver::bolt::BoltDeviceKind`, and
`BatteryStatus`, which mirrors `hidpp 0.2`'s `BatteryStatus`) rather than
importing them from `hidpp` — so the protocol and platform crates never leak
their types upward into core.

`openlogi-hid` and `openlogi-hook` each depend on core and add one capability:
the HID++ protocol and the macOS input hook, respectively. `openlogi-assets` is
a second foundation crate — it has no internal dependencies (not even on core)
and provides the device asset registry schema plus HTTP fetch helpers for the
`assets.openlogi.org` host. `openlogi-cli` and `openlogi-gui` sit at the top and
compose the lower crates into the `openlogi` and `openlogi-gui` binaries: the
CLI builds on core, hid, and assets; the GUI builds on core, hid, hook, and
assets.
