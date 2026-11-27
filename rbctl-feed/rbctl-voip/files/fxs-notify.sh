#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-or-later
# One worker per board owner; EOF means module unload or owner death.
notify() {
	ACTION=change VOIP_NOTIFICATION_TYPE=FXS_STATUS VOIP_FXS_STATUS="$1" /sbin/hotplug-call voip
}
while IFS= read -r state; do
	case "$state" in RINGING|ACTIVE|READY|FAULT) notify "$state" ;; esac
done
notify OFF
