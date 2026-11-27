# rbctl OpenWrt package feed

Register this directory once to build xDSL and VoIP packages for the VX830v. Keep the two source workspaces together: the voice build uses the `af_packet` crate from the DSL workspace, but the voice service does **not** depend on the DSL daemon at runtime.

| Select | What it installs |
|---|---|
| [`rbctl-dsl`](rbctl-dsl/README.md) | xDSL daemon, UCI defaults, procd service and DSL LED integration. |
| [`rbctl-voip`](rbctl-voip/README.md) | Full two-port voice service with the feed's baresip 4.9.0, board module, codecs and FXS LED integration. `rbctl-voip-full` is a metapackage for this profile. |
| `luci-app-rbctl-voip` | Telephony page for SIP accounts and analog ports. Depends on `rbctl-voip` and `luci-base`; the daemon can run without the page. |
| `rbctl-voip-lite` | Alternative G.711 service using the private `baresip-rbctl-mbedtls` runtime. UDP/TCP and plain RTP only; choose this **instead of** the full service. |
| `rbctl-voicectl` | Optional board diagnostics. Stop the voice service before using tools that take control of the board. |

## Build in OpenWrt

Use a buildroot or SDK matching the router image and package ABI (`mediatek/filogic`, `aarch64_cortex-a53` on the tested VX830v). Keep the standard packages, telephony and LuCI feeds available for dependencies. From the buildroot:

```sh
# If there is no feeds.conf yet, start with the buildroot's default feeds.
test -f feeds.conf || cp feeds.conf.default feeds.conf
# Add once to feeds.conf, substituting the absolute path to this checkout:
# src-link rbctl /absolute/path/to/checkout/rbctl-feed
./scripts/feeds update -a
./scripts/feeds install -a
make menuconfig   # Select rbctl-dsl and/or the desired voice packages
```

Compile the selected source package(s):

```sh
make package/feeds/rbctl/rbctl-dsl/compile V=s
make package/feeds/rbctl/rbctl-voip/compile V=s   # also builds LuCI when selected
```

On the tested APK buildroot, outputs are under `bin/packages/<arch>/rbctl/`. For deployment, see the [DSL guide](rbctl-dsl/README.md) or [voice integration guide](../docs/voip/m5-integration.md).
