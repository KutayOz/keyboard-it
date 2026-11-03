#!/usr/bin/env python3
"""Attach the locally release-signed APK after native/Android CI succeeds."""
import hashlib, pathlib, subprocess, sys, tempfile
ROOT=pathlib.Path(__file__).resolve().parents[2]
tag=sys.argv[1] if len(sys.argv)==2 else ''
version=next(line.split('"')[1] for line in (ROOT/'Cargo.toml').read_text().splitlines() if line.startswith('version = '))
if tag!='v'+version:raise SystemExit('Pass the exact workspace tag, e.g. v'+version)
apk=ROOT/'dist/keyboard-it-android.apk'
if not apk.exists():raise SystemExit('Build the stable-key signed release APK first')
# Native installers are created only after all four CI jobs pass. Preserve their
# checksum entries and add the Android artifact with its final download name.
with tempfile.TemporaryDirectory(prefix='keyboard-it-apk-publish-') as temp:
    temp=pathlib.Path(temp)
    subprocess.run(['gh','release','download',tag,'--repo','KutayOz/keyboard-it','--dir',str(temp),'--pattern','SHA256SUMS'],check=True)
    sums=temp/'SHA256SUMS'
    lines=[line for line in sums.read_text().splitlines() if not line.endswith('keyboard-it-android.apk')]
    lines.append(hashlib.sha256(apk.read_bytes()).hexdigest()+'  keyboard-it-android.apk')
    sums.write_text('\n'.join(lines)+'\n')
    subprocess.run(['gh','release','upload',tag,str(apk),str(sums),'--repo','KutayOz/keyboard-it','--clobber'],check=True)
