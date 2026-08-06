# SPDX-License-Identifier: GPL-2.0-only
#
# rbctl-dsl — EcoNet xDSL board configuration daemon (Rust).
# Phase 0: builds the gate binary that proves UCI + uloop + ubus link/load.

include $(TOPDIR)/rules.mk

PKG_NAME:=rbctl-dsl
PKG_VERSION:=0.1.0
PKG_RELEASE:=1

PKG_LICENSE:=GPL-2.0-only
PKG_MAINTAINER:=rbctl-dsl contributors

# Trigger the Rust host toolchain bootstrap (rustc + cargo) on first build.
PKG_BUILD_DEPENDS:=rust/host
PKG_BUILD_PARALLEL:=1

# Virtual workspace at the feed root; the binary crate is a subdirectory.
# Build/Compile/Cargo expands to `cargo install --path $(PKG_BUILD_DIR)/$(MAKE_PATH)`.
MAKE_PATH:=crates/rbctl_dsl

RUST_PKG_LOCKED:=1
CARGO_PKG_PROFILE:=release

include $(INCLUDE_DIR)/package.mk
# External feed: reference rust-package.mk via $(TOPDIR)/feeds/ rather than the
# relative path used by in-tree feed packages (../../lang/rust/...).
include $(TOPDIR)/feeds/packages/lang/rust/rust-package.mk

# Steer the vendored -sys crates at the SDK target staging so they link the
# staged libuci / libubox / libnl-tiny (UCI_DIR / LIBUBOX_DIR / LIBNL_TINY_DIR
# are honoured by libuci-sys, libubox-sys, and tinylibnl_sys respectively).
export UCI_DIR:=$(STAGING_DIR)/usr
export LIBUBOX_DIR:=$(STAGING_DIR)/usr
export LIBNL_TINY_DIR:=$(STAGING_DIR)/usr

# libuci-sys runs bindgen at build time, which invokes libclang. Without
# cross-compile hints, libclang parses the host's glibc headers (x86) and
# trips on __float128. Steer it at the musl toolchain headers instead:
# --target selects the aarch64-musl ABI (no __float128), -isystem points at
# the musl stdint.h / bits/*.h in the SDK toolchain.
export BINDGEN_EXTRA_CLANG_ARGS:=--target=$(RUSTC_TARGET_ARCH) -isystem $(TOOLCHAIN_DIR)/include

# OpenWrt's default Build/Prepare only copies ./src/ into PKG_BUILD_DIR, but
# this package is a Cargo workspace (Cargo.toml + crates/ at the root, not
# ./src/). Override Prepare to copy the full workspace so cargo finds the
# workspace manifest, the lockfile, .cargo/config.toml, and crates/rbctl_dsl/.
define Build/Prepare
	mkdir -p "$(PKG_BUILD_DIR)"
	tar -cf - -C "$(CURDIR)" \
		--exclude=target --exclude=.git --exclude='*.swp' \
		--exclude=files \
		. | tar -xf - -C "$(PKG_BUILD_DIR)"
endef

define Package/rbctl-dsl
  SECTION:=net
  CATEGORY:=Network
  TITLE:=EcoNet xDSL board configuration daemon
  DEPENDS:=$(RUST_ARCH_DEPENDS) +libuci +libubox +libnl-tiny
  # No +libubus here: ubus-zero is pure Rust and links no shared library.
endef

define Package/rbctl-dsl/description
  Configures the EcoNet xDSL board over the proprietary 0x88B5 Ethernet
  protocol and exposes line state over ubus. Replaces the proprietary
  remote_board + libcmm.so stack on mainline OpenWrt.
endef

# RustBinPackage provides a default install rule: PKG_INSTALL_DIR/bin/* -> /usr/bin/.
$(eval $(call RustBinPackage,rbctl-dsl))
$(eval $(call BuildPackage,rbctl-dsl))
