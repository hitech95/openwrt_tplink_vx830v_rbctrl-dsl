# SPDX-License-Identifier: GPL-2.0-or-later
"""Stage a standalone tester APK, without replacing system baresip or libre.

Inputs: target install prefix containing the no-OpenSSL builds of re and
baresip, an ABI-matched production rbctl module, OpenWrt host apk, target
strip, signing key, output directory. See docs/voip/tester-lite.md.
"""
import argparse
import hashlib
import os
from pathlib import Path
import re
import shutil
import subprocess as sp
import tempfile

pkg = Path(__file__).resolve().parents[1]
name, version = 'rbctl-voip-lite', '0.1.0-r2'


def build(a, stage):
    root = stage/'root'
    private = root/'usr/lib/rbctl-voip-lite'
    private.mkdir(parents=True)
    def copy(src, dest, mode):
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(src,dest); dest.chmod(mode)
    for src,dest in [(a.prefix/'bin/baresip',private/'baresip'),
                     (a.prefix/'lib/libre.so.27',private/'libre.so.27'),
                     (a.prefix/'lib/libbaresip.so.18',private/'libbaresip.so.18'),
                     (a.module,private/'modules/rbctl_voip.so')]+[
                     (a.prefix/f'lib/baresip/modules/{m}.so',private/f'modules/{m}.so') for m in ('g711','mwi')]:
        copy(src,dest,0o755)
        sp.run([a.strip,'--strip-unneeded',dest],check=True)
        dynamic=sp.check_output(['readelf','-d',dest],env={**os.environ,'LC_ALL':'C'},text=True)
        needs=set(re.findall(r'\(NEEDED\).*?\[(.*?)\]',dynamic))
        allowed={'libbaresip.so.18','libre.so.27','libmbedcrypto.so.16','libz.so.1','libgcc_s.so.1','libc.so','libubox.so.20260721','libubus.so.20260628'}
        if not needs or needs-allowed or 'RPATH' in dynamic or 'RUNPATH' in dynamic:
            raise ValueError(f'unexpected linkage: {src}: {needs}')
        if dest.name=='libre.so.27' and 'libmbedcrypto.so.16' not in needs:
            raise ValueError('not the mbedcrypto build')
    for src,dest,mode in [('voip.config','etc/config/voip',0o600),('voip.acl.json','usr/share/rpcd/acl.d/rbctl-voip.json',0o644),('render.uc','usr/libexec/rbctl-voip-render.uc',0o644),('voip.init','etc/init.d/voip',0o755)]:
        copy(pkg/'files'/src,root/dest,mode)
    copy(pkg/'files/voip-policy.uc',root/'usr/libexec/voip-policy.uc',0o644)
    copy(pkg/'files/migrate.uc',root/'usr/libexec/rbctl-voip-migrate.uc',0o644)
    copy(pkg/'files/95-voip-policy',root/'etc/uci-defaults/95-voip-policy',0o755)
    (root/'usr/libexec/rbctl-voip-lite.profile').touch()
    info=root/'lib/apk/packages';info.mkdir(parents=True)
    (info/f'{name}.list').write_text(''.join(f'/{f.relative_to(root)}\n' for f in sorted(root.rglob('*')) if f.is_file()))
    (info/f'{name}.conffiles').write_text('/etc/config/voip\n')
    (info/f'{name}.conffiles_static').write_text('/etc/config/voip '+hashlib.sha256((root/'etc/config/voip').read_bytes()).hexdigest()+'\n')
    scripts=[]
    for typ,cmd in [('post-install','default_postinst'),('post-upgrade','export PKG_UPGRADE=1\ndefault_postinst'),('pre-deinstall','default_prerm')]:
        f=stage/typ; f.write_text(f'#!/bin/sh\n[ "${{IPKG_NO_SCRIPT}}" = 1 ] && exit 0\n. "${{IPKG_INSTROOT}}/lib/functions.sh"\nexport root="${{IPKG_INSTROOT}}" pkgname={name}\n{cmd}\n');scripts+=['--script',f'{typ}:{f}']
    deps='!rbctl-voip libc libgcc1 libubus20260628 libubox20260721 libmbedtls21 zlib ubus uci ucode ucode-mod-uci ucode-mod-fs'
    sp.run(['fakeroot',a.apk,'mkpkg','-I','name:'+name,'-I','version:'+version,'-I','arch:aarch64_cortex-a53','-I','license:GPL-2.0-or-later MIT BSD-3-Clause','-I','description:VX830v G.711 voice service (UDP/TCP, plain RTP)','-I','depends:'+deps,'--files',root,*scripts,'--sign-key',a.key,'--output',a.output/f'{name}-{version}.apk'],check=True)


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    for key in ('prefix','module','apk','strip','key','output'): parser.add_argument('--'+key,type=Path,required=True)
    args=parser.parse_args();args.output.mkdir(parents=True,exist_ok=True)
    os.umask(0o022)
    with tempfile.TemporaryDirectory() as tmp: build(args,Path(tmp))
