// SPDX-License-Identifier: GPL-2.0-or-later
import { render } from '/usr/libexec/voip-policy.uc';

return { 'luci.rbctl-voip': { validate: {
	args: { sections: {} },
	call: function(req) {
		try { render(req.args.sections, false, true); }
		catch (e) {
			// Only return our field diagnostics, never interpreter errors or input.
			let m=match(e.message, /^voip: invalid ([a-zA-Z0-9 ._:()\/-]+)\n?$/);
			return {error:m ? m[1] : 'configuration'};
		}
		return {error:''};
	}
} } };
