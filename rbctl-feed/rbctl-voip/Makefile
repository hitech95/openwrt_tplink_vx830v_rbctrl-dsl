# SPDX-License-Identifier: GPL-2.0-or-later
#
# rbctl-voip — VX830v voice board link (Rust): protocol library with a C ABI
# for the baresip module (staged for M3) and the rbctl-voicectl bring-up tool.

include $(TOPDIR)/rules.mk

PKG_NAME:=rbctl-voip
PKG_VERSION:=0.1.0
PKG_RELEASE:=6

# Our crates are GPL-2.0-or-later; af_packet (shared with rbctl-dsl) is MIT.
PKG_LICENSE:=GPL-2.0-or-later MIT
PKG_LICENSE_FILES:=LICENSE
PKG_MAINTAINER:=rbctl-voip contributors

PKG_BUILD_DEPENDS:=rust/host
PKG_CONFIG_DEPENDS:=CONFIG_PACKAGE_baresip-mod-rbctl-voip CONFIG_PACKAGE_rbctl-voip-service-lite
PKG_BUILD_PARALLEL:=1

# Virtual workspace; the installed binary comes from this crate.

RUST_PKG_LOCKED:=1
CARGO_PKG_PROFILE:=release

include $(INCLUDE_DIR)/package.mk
include $(INCLUDE_DIR)/cmake.mk
include $(TOPDIR)/include/rust-package.mk
# Cargo installs the CLI; the extension has its own baresip-mod-* package.
PKG_INSTALL:=

# The buildroot's source-date cache uses CURDIR as part of a make variable
# name; this external feed path contains a space, so compute the same timestamp
# directly. The workspace is copied locally and has no Cargo vendor tree.
PKG_SOURCE_DATE_EPOCH:=$(shell $(TOPDIR)/scripts/get_source_date_epoch.sh "$(CURDIR)")
PKG_SKIP_DOWNLOAD:=1

CMAKE_SOURCE_SUBDIR:=modules/rbctl_voip
CMAKE_BINARY_SUBDIR:=cmake-module
module_selected=$(strip $(CONFIG_PACKAGE_baresip-mod-rbctl-voip)$(CONFIG_PACKAGE_rbctl-voip-service-lite))
CMAKE_OPTIONS+= $(if $(CONFIG_PACKAGE_rbctl-voip-service-lite),-DBARESIP_INCLUDE_DIR=$(STAGING_DIR)/usr/include/rbctl-lite -DRE_INCLUDE_DIR=$(STAGING_DIR)/usr/include/rbctl-lite/re)
CMAKE_OPTIONS+= -DRBCTL_UBUS=ON -DRBCTL_HARNESS=OFF -DBUILD_TESTING=OFF \
	-DVOICE_INCLUDE_DIR="$(PKG_BUILD_DIR)/crates/rbctl_voip_ffi/include" \
	-DVOICE_ARCHIVE="$(PKG_BUILD_DIR)/target/$(RUSTC_TARGET_ARCH)/release/librbctl_voip_ffi.a"

# Same rationale as rbctl-dsl: abort-on-panic is smaller and is the intended
# failure model (the library never panics by construction; see lints).
CARGO_PKG_CONFIG_VARS+=CARGO_PROFILE_RELEASE_PANIC=abort

# af_packet is shared with the sibling rbctl-dsl feed package. Copy it into
# the build tree and pin the fields it inherits from the rbctl-dsl workspace
# so Cargo.lock (--locked) still matches.
AF_PACKET_SRC:=$(CURDIR)/../rbctl-dsl/crates/af_packet
AF_PACKET_VERSION:=$(shell sed -n 's/^version = "\(.*\)"/\1/p' "$(CURDIR)/../rbctl-dsl/Cargo.toml" | head -n1)

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
	$(call Build/Compile/Cargo,crates/rbctl_voicectl)
	+$(CARGO_PKG_VARS) \
	cargo build -v --profile $(CARGO_PKG_PROFILE) $(CARGO_PKG_ARGS) \
		--manifest-path "$(PKG_BUILD_DIR)/Cargo.toml" \
		--target-dir "$(PKG_BUILD_DIR)/target" \
		-p rbctl_voip_ffi
	$(if $(module_selected),$(call Build/Compile/Default))
endef

define Build/Configure
	$(if $(module_selected),$(call Build/Configure/Default))
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
  DEPENDS:=$(RUST_ARCH_DEPENDS) +libgcc
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

define Package/baresip-mod-rbctl-voip
  SECTION:=net
  CATEGORY:=Network
  SUBMENU:=Telephony
  TITLE:=Baresip extension for the VX830v analog voice board
  DEPENDS:=$(RUST_ARCH_DEPENDS) +libgcc +baresip +libre +libubus +libubox
endef

define Package/baresip-mod-rbctl-voip/install
	$(INSTALL_DIR) $(1)/usr/lib/baresip/modules
	$(INSTALL_BIN) $(CMAKE_BINARY_DIR)/rbctl_voip.so $(1)/usr/lib/baresip/modules/
endef

$(eval $(call BuildPackage,baresip-mod-rbctl-voip))

define Package/rbctl-voip
  SECTION:=net
  CATEGORY:=Network
  SUBMENU:=Telephony
  TITLE:=Two-line voice service for the VX830v stock voice board
  DEPENDS:=+baresip-mod-rbctl-voip +rbctl-voip-led +ubus +uci +ucode +ucode-mod-uci +ucode-mod-fs +ca-bundle +baresip-mod-g711 +baresip-mod-g722 +baresip-mod-auresamp +baresip-mod-mwi +baresip-mod-srtp
  CONFLICTS:=rbctl-voip-lite rbctl-voip-service-lite
  EXTRA_DEPENDS:=$(if $(CONFIG_USE_APK),!rbctl-voip-service-lite (>=0))
endef

define Package/rbctl-voip/conffiles
/etc/config/voip
endef

define Package/rbctl-voip/install
	$(INSTALL_DIR) $(1)/usr/libexec $(1)/etc/config $(1)/etc/init.d $(1)/usr/share/rpcd/acl.d
	$(INSTALL_DATA) ./files/render.uc $(1)/usr/libexec/rbctl-voip-render.uc
	$(INSTALL_DATA) ./files/voip-policy.uc $(1)/usr/libexec/
	$(INSTALL_DATA) ./files/migrate.uc $(1)/usr/libexec/rbctl-voip-migrate.uc
	$(INSTALL_DIR) $(1)/etc/uci-defaults
	$(INSTALL_BIN) ./files/95-voip-policy $(1)/etc/uci-defaults/
	$(INSTALL_CONF) ./files/voip.config $(1)/etc/config/voip
	$(INSTALL_BIN) ./files/voip.init $(1)/etc/init.d/voip
	$(INSTALL_DATA) ./files/voip.acl.json $(1)/usr/share/rpcd/acl.d/rbctl-voip.json
endef

$(eval $(call BuildPackage,rbctl-voip))

define Package/rbctl-voip-full
  SECTION:=net
  CATEGORY:=Network
  SUBMENU:=Telephony
  TITLE:=Complete VX830v voice service with panel LEDs
  DEPENDS:=+rbctl-voip
endef

define Package/rbctl-voip-full/install
	true
endef

$(eval $(call BuildPackage,rbctl-voip-full))

define Package/rbctl-voip-service-lite
  SECTION:=net
  CATEGORY:=Network
  SUBMENU:=Telephony
  TITLE:=Managed VX830v G.711 voice service (plain RTP)
  DEPENDS:=$(RUST_ARCH_DEPENDS) +baresip-rbctl-mbedtls +rbctl-voip-led +libubox +libubus +libgcc +ubus +uci +ucode +ucode-mod-uci +ucode-mod-fs
  CONFLICTS:=rbctl-voip
  EXTRA_DEPENDS:=$(if $(CONFIG_USE_APK),!rbctl-voip (>=0))
endef
Package/rbctl-voip-service-lite/conffiles=$(Package/rbctl-voip/conffiles)
define Package/rbctl-voip-service-lite/install
	$(call Package/rbctl-voip/install,$(1))
	$(INSTALL_DIR) $(1)/usr/lib/rbctl-voip-lite/modules
	$(INSTALL_BIN) $(CMAKE_BINARY_DIR)/rbctl_voip.so $(1)/usr/lib/rbctl-voip-lite/modules/
	touch $(1)/usr/libexec/rbctl-voip-lite.profile
endef
$(eval $(call BuildPackage,rbctl-voip-service-lite))

define Package/rbctl-voip-lite
  SECTION:=net
  CATEGORY:=Network
  SUBMENU:=Telephony
  TITLE:=VX830v lite voice profile (mbedcrypto; no SIP/TLS or SRTP)
  DEPENDS:=+rbctl-voip-service-lite
endef
define Package/rbctl-voip-lite/install
	true
endef
$(eval $(call BuildPackage,rbctl-voip-lite))

define Package/luci-app-rbctl-voip
  VERSION:=$(PKG_VERSION)-r7
  SECTION:=luci
  CATEGORY:=LuCI
  SUBMENU:=3. Applications
  TITLE:=LuCI telephony configuration for rbctl-voip
  DEPENDS:=+rbctl-voip +luci-base
endef

define Package/luci-app-rbctl-voip/install
	$(INSTALL_DIR) $(1)/www/luci-static/resources/view/admin/services
	$(INSTALL_DIR) $(1)/usr/share/luci/menu.d $(1)/usr/share/rpcd/acl.d
	$(INSTALL_DATA) ./luci/view/admin/services/rbctl-voip.js $(1)/www/luci-static/resources/view/admin/services/rbctl-voip.js
	$(INSTALL_DATA) ./luci/menu.d/luci-app-rbctl-voip.json $(1)/usr/share/luci/menu.d/luci-app-rbctl-voip.json
	$(INSTALL_DATA) ./luci/acl.d/luci-app-rbctl-voip.json $(1)/usr/share/rpcd/acl.d/luci-app-rbctl-voip.json
	$(INSTALL_DIR) $(1)/usr/share/rpcd/ucode
	$(INSTALL_DATA) ./luci/validate.uc $(1)/usr/share/rpcd/ucode/luci.rbctl-voip
endef

$(eval $(call BuildPackage,luci-app-rbctl-voip))
