'use strict';
'require view';
'require form';
'require uci';
'require rpc';
'require poll';
'require tools.widgets as widgets';

var getStatus = rpc.declare({ object: 'voip', method: 'status', expect: {} });
var getLines = rpc.declare({ object: 'voip', method: 'lines', expect: {} });
var getCalls = rpc.declare({ object: 'voip', method: 'calls', expect: {} });
var check = rpc.declare({ object: 'luci.rbctl-voip', method: 'validate', params: ['sections'], reject: true });

// Validate the parsed candidate, including unsaved edits/deletions, before
// LuCI saves it. Grid edit dialogs have their own Map and save lifecycle.
function checked(map) {
	var save = map.save;
	map.save = function(cb, silent) {
		return save.call(this, function() {
			return Promise.resolve(cb && cb()).then(function() {
				var all = {};
				uci.sections('voip').forEach(function(s) { all[s['.name']] = uci.get('voip', s['.name']); });
				return check(all).then(function(result) {
					if (result.error !== '') throw new Error(_('Invalid VoIP setting: ') + result.error);
				});
			});
		}, false);
	};
}

function safeCall(fn) {
	return fn().catch(function() { return {}; });
}

function sipAddress(account) {
	if (!account.user || !account.domain)
		return _('Incomplete account');

	return 'sip:' + account.user + '@' + account.domain;
}

function callSummary(calls, port) {
	var active = (calls.calls || []).filter(function(call) {
		return call.line == port;
	});

	if (!active.length)
		return _('No call');

	return active.map(function(call) {
		return call.peer_uri || _('Call in progress');
	}).join(', ');
}

return view.extend({
	load: function() {
		return Promise.all([
			uci.load('voip'),
			safeCall(getStatus),
			safeCall(getLines),
			safeCall(getCalls)
		]);
	},

	render: function(data) {
		var status = data[1] || {}, lines = data[2] || {}, calls = data[3] || {};
		var m = new form.Map('voip', _('Telephony'),
			_('Configure SIP accounts and analog phone ports. Saved changes reload the VoIP service.'));
		checked(m);
		m.chain('firewall');
		var s, o;
		m.tabbed = true;

		s = m.section(form.NamedSection, 'board', 'board', _('Board settings'));
		s.addremove = false;
		s.anonymous = false;

		o = s.option(form.Flag, 'enabled', _('Enable telephony'));
		o.default = '1';
		o.rmempty = false;

		o = s.option(form.ListValue, 'locale', _('Tone profile'));
		[ 'AU', 'BE', 'BR', 'CA', 'CN', 'CO', 'ETSI', 'FR', 'DE', 'IN', 'IT',
		  'MX', 'NZ', 'US', 'PE', 'PL', 'PT', 'RU', 'ES', 'TW' ].forEach(function(v) {
			o.value(v, v);
		});
		o.default = 'IT';

		o = s.option(widgets.ZoneSelect, 'firewall_zone', _('SIP firewall zone'));
		o.description = _('Select an existing firewall zone to allow SIP and RTP traffic.');
		o = s.option(form.DummyValue, '_wan_notice', _('WAN exposure'));
		o.depends('firewall_zone', 'wan');
		o.cfgvalue = function() { return _('WAN opens the SIP and RTP ports to the Internet. Keep registrar IP checking enabled and restrict inbound traffic to your provider where possible.'); };

		/* Tabs keep account-level SIP policy separate from physical FXS setup. */
		s = m.section(form.GridSection, 'account', _('SIP accounts'));
		s.addModalOptions = function(section) { checked(section.map); };
		s.handleRemove = function(id) {
			uci.sections('voip', 'line').forEach(function(line) {
				if (line.account === id) uci.unset('voip', line['.name'], 'account');
			});
			return form.GridSection.prototype.handleRemove.apply(this, arguments);
		};
		s.anonymous = true;
		s.addremove = true;
		s.sortable = true;
		s.tab('settings', _('Account settings'));
		s.option(form.DummyValue, '_identity', _('SIP identity')).cfgvalue = function(section) {
			return sipAddress(uci.get('voip', section) || {});
		};
		s.option(form.DummyValue, '_registration', _('Registration')).cfgvalue = function(section) {
			var account = uci.get('voip', section) || {}, aor = sipAddress(account);
			var match = (status.accounts || []).find(function(item) { return item.aor === aor; });
			return account.enabled == '1' ? (match && match.registered ? _('Registered') : _('Not registered')) : _('Disabled');
		};
		s.option(form.DummyValue, '_calls', _('Line activity')).cfgvalue = function(section) {
			var ports = (uci.sections('voip', 'line') || []).filter(function(line) {
				return line.account === section && line.enabled != '0';
			}).map(function(line) { return String(line.port); });
			var active = (lines.lines || []).filter(function(line) {
				return ports.indexOf(String(line.line)) >= 0 && line.state !== 'idle';
			});
			if (!active.length)
				return ports.length ? _('Idle') : _('No assigned active lines');
			return active.map(function(line) {
				return 'FXS ' + (Number(line.line) + 1) + ': ' + line.state + ' — ' + callSummary(calls, line.line);
			}).join('; ');
		};

		o = s.option(form.Flag, 'enabled', _('Enabled'));
		o.default = '0';
		o.rmempty = false;
		o.editable = true;

		o = s.taboption('settings', form.Value, 'display_name', _('Display name'));
		o.modalonly = true;

		o = s.taboption('settings', form.Value, 'user', _('SIP user'));
		o.validate = function(id, v) { return /^[a-zA-Z0-9_.+%-]+$/.test(v) || _('Invalid SIP user'); };
		o.rmempty = false;
		o.modalonly = true;

		o = s.taboption('settings', form.Value, 'domain', _('SIP domain / registrar'));
		o.validate = function(id, v) { return /^(\[[0-9a-fA-F:]+\]|[a-zA-Z0-9.-]+)(:[0-9]+)?$/.test(v) || _('Invalid SIP domain'); };
		o.rmempty = false;
		o.modalonly = true;

		o = s.taboption('settings', form.Value, 'auth_user', _('Authentication user'));
		o.modalonly = true;

		o = s.taboption('settings', form.Value, 'password', _('Password'));
		o.password = true;
		o.modalonly = true;

		o = s.taboption('settings', form.Value, 'outbound', _('Outbound proxy'));
		o.modalonly = true;

		o = s.taboption('settings', form.ListValue, 'transport', _('Transport'));
		o.value('udp', 'UDP');
		o.value('tcp', 'TCP');
		o.value('tls', 'TLS');
		o.default = 'udp';
		o.modalonly = true;

		o = s.taboption('settings', form.MultiValue, 'codec', _('Audio codecs'));
		o.value('PCMA', 'G.711 A-law (PCMA)');
		o.value('PCMU', 'G.711 μ-law (PCMU)');
		o.value('G722', 'G.722');
		o.default = 'PCMA';
		o.modalonly = true;

		o = s.taboption('settings', form.ListValue, 'dtmf', _('DTMF mode'));
		o.value('rfc4733', _('RTP events (RFC 4733)'));
		o.value('info', _('SIP INFO'));
		o.value('inband', _('In-band audio'));
		o.default = 'rfc4733';
		o.modalonly = true;

		o = s.taboption('settings', form.ListValue, 'srtp', _('Media encryption'));
		o.value('off', _('Off'));
		o.value('optional', _('Optional SRTP'));
		o.value('mandatory', _('Require SRTP'));
		o.default = 'off';
		o.modalonly = true;

		o = s.taboption('settings', form.Value, 'expires', _('Registration interval (seconds)'));
		o.datatype = 'range(0,86400)';
		o.default = '3600';
		o.modalonly = true;

		o = s.taboption('settings', form.ListValue, 'incoming', _('Ring group behavior'));
		o.value('ring_all', _('Ring all assigned lines'));
		o.value('hunt', _('Ring first free line'));
		o.description = _('Hunt checks PHONE1, then PHONE2. It rings only the first free, enabled port assigned to this account.');
		o.default = 'ring_all';
		o.modalonly = true;
		o = s.taboption('settings', form.Flag, 'check_origin', _('Accept incoming calls only from the registrar IP'));
		o.default = '1'; o.rmempty = false; o.modalonly = true;

		o = s.taboption('settings', form.Value, 'digitmap', _('Dial plan'));
		o.default = '[0-9*]+';
		o.description = _('Pattern of permitted digits. The # key sends digits immediately.');
		o.modalonly = true;
		o = s.taboption('settings', form.Value, 'interdigit_timeout', _('Short inter-digit timeout (seconds)'));
		o.datatype = 'and(uinteger,range(1,600))'; o.placeholder = '4'; o.modalonly = true;
		o.description = _('Wait this long before dialing a complete number that could still match more digits.');
		o = s.taboption('settings', form.Value, 'interdigit_long_timeout', _('Long inter-digit timeout (seconds)'));
		o.datatype = 'and(uinteger,range(1,600))'; o.placeholder = '10'; o.modalonly = true;
		o.description = _('Wait this long for another digit when the number is still incomplete.');
		o = s.taboption('settings', form.Flag, 'require_registration', _('Require registration for dial tone'));
		o.default = '0'; o.rmempty = false; o.modalonly = true;
		o = s.taboption('settings', form.Value, 'alert_info', _('Alert-Info ring mapping'));
		o.placeholder = 'internal=2'; o.modalonly = true;
		o.description = _('Map one provider Alert-Info value to a ring cadence (0–9). Leave blank for the default ring.');
		o.validate = function(id, v) { return !v || /^[a-zA-Z0-9_.-]{1,79}=[0-9]$/.test(v) || _('Use one value=number mapping, for example internal=2.'); };
		o.write = function(id, v) { uci.set('voip', id, 'alert_info', [v]); };

		/* FXS ports are a fixed pair. GridSection supplies a per-row Edit modal. */
		s = m.section(form.GridSection, 'line', _('Analog phone ports'));
		s.addModalOptions = function(section) { checked(section.map); };
		s.anonymous = true;
		s.addremove = false;
		s.sortable = false;
		s.tab('settings', _('Port settings'));
		s.option(form.DummyValue, '_portname', _('Port')).cfgvalue = function(section) {
			return uci.get('voip', section, 'port') == '1' ? 'PHONE2' : 'PHONE1';
		};
		o = s.option(form.Flag, 'enabled', _('Enabled'));
		o.default = '1';
		o.editable = true;
		o.rmempty = false;
		s.option(form.DummyValue, '_state', _('Line state')).cfgvalue = function(section) {
			var port = uci.get('voip', section, 'port');
			var item = (lines.lines || []).find(function(line) { return String(line.line) === String(port); });
			return item ? item.state : _('Unknown');
		};
		s.option(form.DummyValue, '_account', _('SIP account')).cfgvalue = function(section) {
			var account = uci.get('voip', section, 'account');
			var s = account && uci.get('voip', account);
			return s ? sipAddress(s) : _('Unassigned');
		};

		o = s.taboption('settings', form.ListValue, 'account', _('SIP account'));
		o.load = function(id) {
			this.keylist = []; this.vallist = [];
			this.value('', _('Unassigned'));
			uci.sections('voip', 'account').forEach(a => this.value(a['.name'], sipAddress(a)));
			return form.ListValue.prototype.load.call(this, id);
		};
		o.modalonly = true;

		o = s.taboption('settings', form.Flag, 'cid', _('Caller ID'));
		o.default = '1'; o.modalonly = true; o.rmempty = false;
		o = s.taboption('settings', form.Flag, 'call_waiting', _('Call waiting'));
		o.default = '1'; o.modalonly = true; o.rmempty = false;
		o = s.taboption('settings', form.Flag, 'mwi', _('Message waiting indication'));
		o.default = '1'; o.modalonly = true; o.rmempty = false;
		o = s.taboption('settings', form.Value, 'tx_gain', _('Transmit gain'));
		o.datatype = 'range(-6,6)'; o.default = '0'; o.modalonly = true;
		o = s.taboption('settings', form.Value, 'rx_gain', _('Receive gain'));
		o.datatype = 'range(-6,6)'; o.default = '0'; o.modalonly = true;

		poll.add(function() {
			return Promise.all([ safeCall(getStatus), safeCall(getLines), safeCall(getCalls) ]).then(function(current) {
				var accountStatus = current[0] || {}, lineStatus = current[1] || {}, callStatus = current[2] || {};
					document.querySelectorAll('#cbi-voip-account tr[data-sid]').forEach(function(row) {
					var sid = row.getAttribute('data-sid'), account = uci.get('voip', sid) || {};
					var address = sipAddress(account);
					var registered = (accountStatus.accounts || []).some(function(item) { return item.aor === address && item.registered; });
					var reg = row.querySelector('[data-name="_registration"]');
					if (reg) reg.textContent = account.enabled == '1' ? (registered ? _('Registered') : _('Not registered')) : _('Disabled');
					var activity = row.querySelector('[data-name="_calls"]');
					if (activity) {
						var ports = (uci.sections('voip', 'line') || []).filter(function(line) { return line.account === sid && line.enabled != '0'; }).map(function(line) { return String(line.port); });
						var active = (lineStatus.lines || []).filter(function(line) { return ports.indexOf(String(line.line)) >= 0 && line.state !== 'idle'; });
						activity.textContent = !active.length ? (ports.length ? _('Idle') : _('No assigned active lines')) : active.map(function(line) { return 'FXS ' + (Number(line.line) + 1) + ': ' + line.state + ' — ' + callSummary(callStatus, line.line); }).join('; ');
					}
				});
				document.querySelectorAll('#cbi-voip-line tr[data-sid]').forEach(function(row) {
					var sid = row.getAttribute('data-sid'), port = uci.get('voip', sid, 'port');
					var item = (lineStatus.lines || []).find(function(line) { return String(line.line) === String(port); });
					var state = row.querySelector('[data-name="_state"]');
					if (state) state.textContent = item ? item.state : _('Unknown');
				});
			});
		}, 5);

		return m.render();
	}
});
