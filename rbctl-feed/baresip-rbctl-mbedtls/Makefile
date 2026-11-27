# SPDX-License-Identifier: GPL-2.0-or-later
include $(TOPDIR)/rules.mk

PKG_NAME:=baresip-rbctl-mbedtls
PKG_VERSION:=4.9.0
PKG_RELEASE:=1
PKG_LICENSE:=BSD-3-Clause
PKG_SOURCE:=baresip-$(PKG_VERSION).tar.gz
PKG_SOURCE_URL:=https://codeload.github.com/baresip/baresip/tar.gz/v$(PKG_VERSION)?
PKG_HASH:=fe0dc70640616b5b6814f728af3ce83b4c56dd1d7a9a21658480c305bd092367
PKG_BUILD_PARALLEL:=1

include $(INCLUDE_DIR)/package.mk
include $(INCLUDE_DIR)/cmake.mk
PKG_SOURCE_DATE_EPOCH:=$(shell $(TOPDIR)/scripts/get_source_date_epoch.sh "$(CURDIR)")

define Download/rbctl-re
  FILE:=re-$(PKG_VERSION).tar.gz
  URL:=https://codeload.github.com/baresip/re/tar.gz/v$(PKG_VERSION)?
  HASH:=2094822477e4b68bf03089fe29b43b5160103ba846847004df25f9e3d65fabb6
endef
$(eval $(call Download,rbctl-re))

define Package/baresip-rbctl-mbedtls
  SECTION:=net
  CATEGORY:=Network
  SUBMENU:=Telephony
  TITLE:=Private G.711 baresip runtime using mbedcrypto (UDP/TCP only)
  DEPENDS:=+libmbedtls +zlib +libgcc
endef

define Build/Prepare
	mkdir -p $(PKG_BUILD_DIR)/bs $(PKG_BUILD_DIR)/re
	tar xzf $(DL_DIR)/$(PKG_SOURCE) -C $(PKG_BUILD_DIR)/bs --strip-components=1
	tar xzf $(DL_DIR)/re-$(PKG_VERSION).tar.gz -C $(PKG_BUILD_DIR)/re --strip-components=1
	patch -d $(PKG_BUILD_DIR)/re -p1 < "$(CURDIR)/files/getrandom.patch"
endef

private_prefix=$(PKG_BUILD_DIR)/private/usr
CMAKE_OPTIONS+= -DUSE_OPENSSL=OFF -DUSE_MBEDTLS=ON -DUSE_BFCP=OFF -DUSE_RTMP=OFF -DLIBRE_BUILD_STATIC=OFF -DCMAKE_INSTALL_LIBDIR=lib
# Each invocation uses OpenWrt's cross compiler, sysroot and normal CMake flags.
CMAKE_SOURCE_SUBDIR=$(part)
CMAKE_BINARY_SUBDIR=build-$(part)
CMAKE_OPTIONS+= $(if $(filter re,$(part)),-DCMAKE_SHARED_LINKER_FLAGS="$(TARGET_LDFLAGS) -Wl$(comma)--no-as-needed -lmbedcrypto -Wl$(comma)--as-needed",'-DMODULES=g711;mwi' -DCMAKE_MODULE_PATH=$(PKG_BUILD_DIR)/re/cmake -Dre_DIR=$(private_prefix)/lib/cmake/re -DRE_INCLUDE_DIR=$(private_prefix)/include/re -DRE_LIBRARY=$(private_prefix)/lib/libre.so)
define Build/Configure
	$(foreach part,re,$(call Build/Configure/Default))
endef
define Build/Compile
	$(foreach part,re,$(call Build/Compile/Default))
	DESTDIR=$(PKG_BUILD_DIR)/private cmake --install $(PKG_BUILD_DIR)/build-re
	$(foreach part,bs,$(call Build/Configure/Default))
	$(foreach part,bs,$(call Build/Compile/Default))
endef
define Build/Install
	DESTDIR=$(PKG_BUILD_DIR)/private cmake --install $(PKG_BUILD_DIR)/build-bs
endef
define Build/InstallDev
	$(INSTALL_DIR) $(1)/usr/include/rbctl-lite
	$(CP) $(private_prefix)/include/. $(1)/usr/include/rbctl-lite/
endef

private_dir=/usr/lib/rbctl-voip-lite
define Package/baresip-rbctl-mbedtls/install
	$(INSTALL_DIR) $(1)$(private_dir)/modules $(1)/usr/share/licenses/$(PKG_NAME)
	$(INSTALL_BIN) $(private_prefix)/bin/baresip $(1)$(private_dir)/
	$(CP) $(private_prefix)/lib/libre.so.* $(private_prefix)/lib/libbaresip.so.* $(1)$(private_dir)/
	$(INSTALL_BIN) $(private_prefix)/lib/baresip/modules/{g711,mwi}.so $(1)$(private_dir)/modules/
	$(INSTALL_DATA) $(PKG_BUILD_DIR)/re/LICENSE $(1)/usr/share/licenses/$(PKG_NAME)/re
	$(INSTALL_DATA) $(PKG_BUILD_DIR)/bs/LICENSE $(1)/usr/share/licenses/$(PKG_NAME)/baresip
endef
$(eval $(call BuildPackage,baresip-rbctl-mbedtls))
