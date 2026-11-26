# SPDX-License-Identifier: GPL-2.0-or-later
#
# rbctl-voip — VX830v voice board link (Rust): protocol library with a C ABI
# for the baresip module (staged for M3) and the rbctl-voicectl bring-up tool.

include $(TOPDIR)/rules.mk

PKG_NAME:=rbctl-voip
PKG_VERSION:=0.1.0
PKG_RELEASE:=1

# Our crates are GPL-2.0-or-later; af_packet (shared with rbctl-dsl) is MIT.
PKG_LICENSE:=GPL-2.0-or-later MIT
PKG_LICENSE_FILES:=LICENSE
PKG_MAINTAINER:=rbctl-voip contributors

PKG_BUILD_DEPENDS:=rust/host
PKG_BUILD_PARALLEL:=1

# Virtual workspace; the installed binary comes from this crate.
MAKE_PATH:=crates/rbctl_voicectl

RUST_PKG_LOCKED:=1
CARGO_PKG_PROFILE:=release

include $(INCLUDE_DIR)/package.mk
include $(TOPDIR)/feeds/packages/lang/rust/rust-package.mk

# Same rationale as rbctl-dsl: abort-on-panic is smaller and is the intended
# failure model (the library never panics by construction; see lints).
CARGO_PKG_CONFIG_VARS+=CARGO_PROFILE_RELEASE_PANIC=abort

# af_packet is shared with the sibling rbctl-dsl feed package. Copy it into
# the build tree and pin the fields it inherits from the rbctl-dsl workspace
# so Cargo.lock (--locked) still matches.
AF_PACKET_SRC:=$(CURDIR)/../rbctl-dsl/crates/af_packet
AF_PACKET_VERSION:=$(shell sed -n 's/^version = "\(.*\)"/\1/p' $(CURDIR)/../rbctl-dsl/Cargo.toml | head -n1)

define Build/Prepare
	mkdir -p "$(PKG_BUILD_DIR)"
	tar -cf - -C "$(CURDIR)" \
		--exclude=target --exclude=.git --exclude='*.swp' --exclude=files \
		--exclude=fuzz \
		. | tar -xf - -C "$(PKG_BUILD_DIR)"
	mkdir -p "$(PKG_BUILD_DIR)/vendor-local"
	cp -a "$(AF_PACKET_SRC)" "$(PKG_BUILD_DIR)/vendor-local/af_packet"
	sed -i \
		-e 's/^version.workspace = true/version = "$(AF_PACKET_VERSION)"/' \
		-e 's/^edition.workspace = true/edition = "2021"/' \
		-e 's/^rust-version.workspace = true/rust-version = "1.80"/' \
		"$(PKG_BUILD_DIR)/vendor-local/af_packet/Cargo.toml"
	sed -i 's#"../rbctl-dsl/crates/af_packet"#"vendor-local/af_packet"#' \
		"$(PKG_BUILD_DIR)/Cargo.toml"
endef

# Install the CLI, then build the C ABI staticlib for the baresip module.
define Build/Compile
	$(call Build/Compile/Cargo)
	+$(CARGO_PKG_VARS) \
	cargo build -v --profile $(CARGO_PKG_PROFILE) $(CARGO_PKG_ARGS) \
		--manifest-path "$(PKG_BUILD_DIR)/Cargo.toml" \
		--target-dir "$(PKG_BUILD_DIR)/target" \
		-p rbctl_voip_ffi
endef

# Staged for the M3 baresip module build (not installed on the target).
define Build/InstallDev
	$(INSTALL_DIR) $(1)/usr/include $(1)/usr/lib
	$(INSTALL_DATA) $(PKG_BUILD_DIR)/crates/rbctl_voip_ffi/include/rbctl_voip.h $(1)/usr/include/
	$(INSTALL_DATA) $(PKG_BUILD_DIR)/target/$(RUSTC_TARGET_ARCH)/release/librbctl_voip_ffi.a $(1)/usr/lib/
endef

define Package/rbctl-voicectl
  SECTION:=net
  CATEGORY:=Network
  SUBMENU:=Telephony
  TITLE:=VX830v voice board bring-up and diagnostic tool
  DEPENDS:=$(RUST_ARCH_DEPENDS)
endef

define Package/rbctl-voicectl/description
  Talks to the stock Econet voice board over raw Ethernet (EtherTypes
  0xDD01-0xDD03 on the management VLAN): initialization, hook and keypad
  events, ringing with caller ID, tones and a two-line audio loop.
  Diagnostic tool; requires CAP_NET_RAW.
endef

define Package/rbctl-voicectl/install
	$(INSTALL_DIR) $(1)/usr/sbin
	$(INSTALL_BIN) $(PKG_INSTALL_DIR)/bin/rbctl-voicectl $(1)/usr/sbin/
endef

$(eval $(call RustBinPackage,rbctl-voicectl))
$(eval $(call BuildPackage,rbctl-voicectl))

define Package/rbctl-voip-led
  SECTION:=net
  CATEGORY:=Network
  SUBMENU:=Telephony
  TITLE:=OpenWrt front-panel FXS LED hotplug integration
endef

define Package/rbctl-voip-led/install
	$(INSTALL_DIR) $(1)/usr/libexec $(1)/etc/hotplug.d/voip
	$(INSTALL_BIN) ./files/fxs-notify.sh $(1)/usr/libexec/rbctl-fxs-notify
	$(INSTALL_DATA) ./files/led_fxs.sh $(1)/etc/hotplug.d/voip/50-led-fxs
endef

$(eval $(call BuildPackage,rbctl-voip-led))
