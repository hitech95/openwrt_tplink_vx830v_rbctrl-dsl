#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-or-later
[ "$VOIP_NOTIFICATION_TYPE" = FXS_STATUS ] || exit 0
. /lib/functions.sh
. /lib/functions/leds.sh
config_load system
config_get green led_fxs sysfs 'green:fxs'
config_get red led_fxs_fault sysfs 'red:fxs'
case "$VOIP_FXS_STATUS" in
	RINGING) led_off "$red"; led_timer "$green" 200 200 ;;
	ACTIVE)  led_off "$red"; led_timer "$green" 500 500 ;;
	READY)   led_off "$red"; led_on "$green" ;;
	FAULT)   led_off "$green"; led_on "$red" ;;
	OFF)     led_off "$green"; led_off "$red" ;;
esac
