#!/bin/sh
#
# led_dsl.sh — DSL LED hotplug handler for rbctl-dsl.
#
# Triggered by /sbin/hotplug-call dsl (via dsl_notify.sh) whenever the
# daemon emits a DSL_INTERFACE_STATUS event. Drives the UCI-configured
# 'led_dsl' LED:
#
#   HANDSHAKE  → slow blink (500 ms on / 500 ms off)
#   TRAINING   → fast blink (200 ms on / 200 ms off)
#   UP         → solid on (or netdev trigger if configured)
#   * (DOWN)   → off
#
# UCI LED configuration (system config):
#
#   config led 'led_dsl'
#       option sysfs   '<sysfs-name>'     # e.g. 'inet' or 'wan'
#       option trigger 'netdev'           # optional: use netdev instead of on/off
#       option dev     'dsl0'             # for netdev trigger
#       list mode      'link'             # 'link'/'tx'/'rx' for netdev

[ "$DSL_NOTIFICATION_TYPE" = "DSL_INTERFACE_STATUS" ] || exit 0

. /lib/functions.sh
. /lib/functions/leds.sh

led_dsl_up() {
	case "$(config_get led_dsl trigger)" in
	"netdev")
		led_set_attr "$1" "trigger" "netdev"
		led_set_attr "$1" "device_name" "$(config_get led_dsl dev)"
		for m in $(config_get led_dsl mode); do
			led_set_attr "$1" "$m" "1"
		done
		;;
	*)
		led_on "$1"
		;;
	esac
}

config_load system
led="$(config_get led_dsl sysfs)"

[ -n "$led" ] || exit 0

case "$DSL_INTERFACE_STATUS" in
	"HANDSHAKE")  led_timer "$led" 500 500 ;;
	"TRAINING")   led_timer "$led" 200 200 ;;
	"UP")         led_dsl_up "$led" ;;
	*)            led_off "$led" ;;
esac
