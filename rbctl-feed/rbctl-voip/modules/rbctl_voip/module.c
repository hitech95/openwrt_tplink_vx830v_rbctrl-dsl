/* SPDX-License-Identifier: GPL-2.0-or-later */
#include <errno.h>
#include <re.h>
#include <baresip.h>
#include "attach.h"
#include "phone.h"
#include "panel.h"

static int module_init(void)
{
	char device[16] = "lan0.500";
	uint32_t locale = 10; /* Italy, as verified with the VX830v handsets */
	int err = conf_get_str(conf_cur(), "rbctl_voip_device", device, sizeof(device));
	if (err && err != ENOENT)
		return err;
	err = conf_get_u32(conf_cur(), "rbctl_voip_locale", &locale);
	if (err && err != ENOENT)
		return err;
	uint32_t line = 0;
	err = conf_get_u32(conf_cur(), "rbctl_voip_line", &line);
	if (err && err != ENOENT) return err;
	if (line > 1) return EINVAL;
	err = phone_init(line);
	if (err) return err;
	err = voice_attach_start(device, locale);
	if (err) phone_close();
	else panel_start();
	return err;
}

static int module_close(void)
{
	panel_close();
	voice_attach_stop();
	phone_close();
	return 0;
}

EXPORT_SYM const struct mod_export DECL_EXPORTS(rbctl_voip) = {
	"rbctl_voip", "application", module_init, module_close
};
