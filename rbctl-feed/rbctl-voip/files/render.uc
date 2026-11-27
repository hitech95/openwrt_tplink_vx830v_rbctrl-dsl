// SPDX-License-Identifier: GPL-2.0-or-later
import * as fs from 'fs';
import * as uci from 'uci';
import { render } from './voip-policy.uc';

let cur=uci.cursor(ARGV[1] || '/etc/config');
let files=render(cur.get_all('voip'), ARGV[2]=='lite', false);
for (let name,data in files)
	if (fs.writefile(`${ARGV[0]}/${name}`,data) !== length(data)) die(`voip: cannot write ${name}\n`);
