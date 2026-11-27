// SPDX-License-Identifier: GPL-2.0-or-later
import * as uci from 'uci';
let cur=uci.cursor(ARGV[0] || '/etc/config'), all=cur.get_all('voip'), changed=false;
// Existing account policy wins; otherwise the lowest numbered port wins.
for (let port in ['0','1']) for (let id,l in all) {
	if (l['.type']!='line' || l.port!=port || !l.account) continue;
	let a=all[l.account];
	if (a?.['.type']!='account') continue;
	for (let k in ['incoming','digitmap','interdigit_timeout','interdigit_long_timeout','alert_info','require_registration']) {
		if (l[k]==null) continue;
		if (a[k]==null) {
			if (!cur.set('voip',l.account,k,l[k])) die('voip: migration set failed\n');
			a[k]=l[k];
		} else if (sprintf('%J',a[k])!=sprintf('%J',l[k])) warn(`voip: conflicting line ${k} on port ${port}; keeping account value\n`);
		if (!cur.delete('voip',id,k)) die('voip: migration delete failed\n');
		changed=true;
	}
}
if (changed && !cur.commit('voip')) die('voip: migration commit failed\n');
