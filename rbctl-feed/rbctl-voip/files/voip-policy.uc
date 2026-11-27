// SPDX-License-Identifier: GPL-2.0-or-later
// Render into a new private directory. Never print values (accounts contain secrets).
export function render(sections, lite, strict) {
function bad(k) { die(`voip: invalid ${k}\n`); }
function text(s,k,d,pattern) {
	let v = s[k] ?? d;
	if (type(v) != 'string' || !match(v, pattern) || match(v, /[\r\n]/)) bad(k);
	return v;
}
function num(s,k,d,min,max) {
	let v = text(s,k,`${d}`,/^-?[0-9]+$/), n = int(v,10);
	if (n < min || n > max) bad(k);
	return n;
}
function choice(s,k,d,opts) {
	let v = s[k] ?? d;
	if (index(opts,v) < 0) bad(k);
	return v;
}
function series(v) { return type(v)=='array' ? v : [v]; }
let b = sections?.board;
if (!b || b['.type'] != 'board') bad('board');
let dev=text(b,'device','lan0',/^[a-zA-Z0-9_-]+$/), vid=num(b,'vid',500,1,4094);
// Host command indices, NOT the SLIC hardware profile IDs. Index 18 has
// no verified country name (see docs/voip/peer.md); do not guess one.
let profiles={ AU:0, BE:1, BR:2, CA:3, CN:4, CO:5, ETSI:6, FR:7,
	DE:8, IN:9, IT:10, MX:11, NZ:12, US:13, PE:14, PL:15, PT:16,
	RU:17, ES:19, TW:20 };
let name=uc(text(b,'locale','IT',/^[a-zA-Z]+$/));
if (!(name in profiles)) bad('board.locale (use a supported country code or ETSI)');
let iface=`${dev}.${vid}`, locale=profiles[name];
if (length(iface)>15) bad('device');
let clk=choice(b,'playout_clock','timer',['timer','board']);
choice(b,'board_codec','pcma',['pcma']); num(b,'ptime',20,20,20);
choice(b,'log_level','info',['info']);
let module_path=lite ? '/usr/lib/rbctl-voip-lite/modules' : '/usr/lib/baresip/modules';
let base=`module_path ${module_path}\nrbctl_voip_managed 1\nrbctl_voip_device ${iface}\nrbctl_voip_locale ${locale}\nrbctl_voip_playout_clock ${clk}\n` +
	'audio_source rbctl_voip,phone\naudio_player rbctl_voip,phone\naudio_txmode poll\naudio_jitter_buffer_delay 2-8\nausrc_srate 8000\nauplay_srate 8000\n' +
	'module g711.so\n'+(lite ? '' : 'module g722.so\nmodule auresamp.so\nmodule srtp.so\n')+'module_app mwi.so\nmodule_app rbctl_voip.so\n';
let zone=text(b,'firewall_zone','',/^[a-zA-Z0-9_]*$/);
let vlan=num(b,'manage_vlan',1,0,1);
base+=`rbctl_voip_init_timeout ${num(b,'init_timeout',5,1,120)}\n`;
base+=lite ? 'sip_transports udp,tcp\n' : 'sip_verify_server yes\nsip_cafile /etc/ssl/certs/ca-certificates.crt\n';
base+='rtp_ports 10000-20000\nfilter_registrar udp,tcp,tls\n';
if (zone) base+='sip_listen 0.0.0.0:5060\n';
let accounts=[], aors={}, used={}, ports={}, policy='';
for (let id,s in sections) {
	if (s['.type']!='account' || !num(s,'enabled',1,0,1)) continue;
	if (!s.user || !s.domain) {
		if (strict) bad('user/domain (required for enabled accounts)');
		warn('voip: skipping incomplete enabled account\n'); continue;
	}
	if (length(accounts)>=32) bad('account count');
	let user=text(s,'user','',/^[a-zA-Z0-9_.+%-]+$/);
	let domain=text(s,'domain','',/^(\[[0-9a-fA-F:]+\]|[a-zA-Z0-9.-]+)(:[0-9]+)?$/);
	let aor=`sip:${user}@${domain}`, tr=choice(s,'transport','udp',['udp','tcp','tls']);
	if (lite && tr=='tls') bad('transport: TLS requires the full package');
	if (used[aor] || length(aor)>240) bad('duplicate or long AOR');
	used[aor]=true; aors[id]=aor;
	let display=text(s,'display_name','',/^[^"<>[:cntrl:]]*$/);
	if (index(display,'\\')>=0) bad('display_name');
	let acc=(display ? `"${display}" ` : '')+`<${aor};transport=${tr}>`;
	acc+=`;check_origin=${num(s,'check_origin',1,0,1)?'yes':'no'}`;
	for (let k in ['auth_user','password']) {
		let v=text(s,k,'',/^[^;"<>[:space:][:cntrl:]]*$/);
		if (index(v,'\\')>=0) bad(k);
		if (v) acc+=`;${k=='password'?'auth_pass':k}=${v}`;
	}
	if (s.auth_user && !s.password) bad('password');
	let proxy=text(s,'outbound',s.registrar || '',/^(sips?:[a-zA-Z0-9.:[\]_-]+(;transport=(udp|tcp|tls))?)?$/);
	if (s.registrar && s.outbound && s.registrar!=s.outbound) bad('distinct registrar and outbound');
	if (lite && match(proxy,/^(sips:)|;transport=tls$/)) bad('encrypted outbound: use the full package');
	if (proxy) acc+=`;outbound="${proxy}"`;
	let codecs=series(s.codec ?? 'PCMA'), mapped=[];
	for (let c in codecs) {
		if (lite && c=='G722') bad('codec: G722 requires the full package');
		if (index(['PCMA','PCMU','G722'],c)<0) bad('codec');
		push(mapped,c=='G722'?'G722/16000/1':`${c}/8000/1`);
	}
	let enc=choice(s,'srtp','off',['off','optional','mandatory']);
	if (lite && enc!='off') bad('srtp: encrypted media requires the full package');
	if (enc!='off') acc+=`;mediaenc=${enc=='mandatory'?'srtp-mand':'srtp'}`;
	let dtmf=choice(s,'dtmf','rfc4733',['rfc4733','info','inband']);
	// Baresip only uses select_audio()'s per-dialog device choice if the
	// account also specifies the module. Seed from a mapped line; a shared
	// account's device is selected again immediately before connect/accept.
	for (let _,l in sections) {
		if (l['.type']!='line' || l.account!=id || !num(l,'enabled',1,0,1)) continue;
		let port=num(l,'port',-1,0,1);
		acc+=`;audio_source=rbctl_voip,phone${port};audio_player=rbctl_voip,phone${port}`;
		break;
	}
	acc+=`;audio_codecs=${join(',',mapped)};dtmfmode=${dtmf=='info'?'info':'rtpevent'};regint=${num(s,'expires',3600,0,86400)}`;
	let mwi=false;
	for (let _,l in sections) if (l['.type']=='line' && l.account==id && num(l,'enabled',1,0,1) && num(l,'mwi',1,0,1)) mwi=true;
	acc+=`;mwi=${mwi?'yes':'no'}`;
	if (length(acc)>=1023) bad('account length');
	push(accounts,acc);
}
for (let id,s in sections) {
	if (s['.type']!='line' || !num(s,'enabled',1,0,1)) continue;
	let port=num(s,'port',-1,0,1), a=s.account ? aors[s.account] : null;
	if (!a) { warn(`voip: skipping port ${port} without an active account\n`); continue; }
	if (ports[port]) bad('duplicate port');
	ports[port]=true;
	// FLASH is a decoded stock-board event, not a host-timed hook pulse.
	num(s,'flash_min_ms',100,100,100); num(s,'flash_max_ms',800,800,800);
	function opt(k,v) { policy+=`rbctl_voip_${k}${port} ${v}\n`; }
	opt('account',a);
	let account=sections[s.account], effective={};
	for (let k in ['digitmap','incoming','interdigit_timeout','interdigit_long_timeout','require_registration','alert_info']) {
		if (account[k] != null) effective[k]=account[k];
		else if (s[k] != null) {
			warn(`voip: deprecated line ${k} on port ${port}; move it to the account\n`);
			effective[k]=s[k];
		}
	}
	let dm=text(effective,'digitmap','[0-9*]+',/^[^[:space:][:cntrl:]]{1,250}$/);
	let rules=split(dm,'|');
	if (length(rules)>16) bad('digitmap');
	for (let rule in rules) {
		if (!match(rule,/^([0-9xX*#]|\[([0-9*#]|[0-9]-[0-9])+\]){1,32}[.+]?T?$/)) bad('digitmap');
		for (let j=1;j<length(rule)-1;j++) if (substr(rule,j,1)=='-' && substr(rule,j-1,1)>substr(rule,j+1,1)) bad('digitmap');
	}
	opt('digitmap',dm);
	for (let spec in [['first_digit','first_digit_timeout',15],['short_digit','interdigit_timeout',4],['long_digit','interdigit_long_timeout',10],['offhook_warning','offhook_warning_timeout',60],['howler_duration','howler_duration',15]])
		opt(spec[0],num(index(spec[1],'interdigit')==0?effective:s,spec[1],spec[2],1,600));
	opt('require_registration',num(effective,'require_registration',0,0,1));
	for (let k in ['cid','call_waiting','mwi','cw_cid']) opt(k,num(s,k,index(['cid','call_waiting','mwi'],k)>=0?1:0,0,1));
	for (let k in ['tx_gain','rx_gain']) opt(k,num(s,k,0,-6,6));
	opt('hunt',choice(effective,'incoming','ring_all',['ring_all','hunt'])=='hunt'?1:0);
	opt('inband',sections[s.account].dtmf=='inband'?1:0);
	if (effective.alert_info) {
		let rules=series(effective.alert_info), m=match(rules[0],/^([a-zA-Z0-9_.-]{1,79})=([0-9])$/);
		if (length(rules)!=1 || !m) bad('alert_info');
		opt('alert_info',m[1]); opt('alert_entry',m[2]);
	}
}
return {board:base+`# vlan=${vlan} firewall_zone=${zone}\n`, config:base+policy,
	device:iface, manage_vlan:`${vlan}`, zone, accounts:length(accounts)?join('\n',accounts)+'\n':''};
}
