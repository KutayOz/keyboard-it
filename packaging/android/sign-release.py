#!/usr/bin/env python3
"""Build with a stable local signing identity; never upload private keys to CI."""
import hashlib, json, os, pathlib, secrets, shutil, subprocess
ROOT = pathlib.Path(__file__).resolve().parents[2]
PRIVATE = ROOT / '.android-signing'
PRIVATE.mkdir(mode=0o700, exist_ok=True)
SETTINGS = PRIVATE / 'local.json'
if SETTINGS.exists():
    config = json.loads(SETTINGS.read_text())
else:
    config = {'password': secrets.token_hex(32)}
    fd = os.open(SETTINGS, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, 'w') as stream:
        json.dump(config, stream)
key = PRIVATE / 'release.p12'
env = dict(os.environ, KEYBOARD_IT_ANDROID_KEYSTORE=str(key),
           KEYBOARD_IT_ANDROID_STORE_PASSWORD=config['password'], KEYBOARD_IT_ANDROID_KEY_PASSWORD=config['password'])
java_home = env.get('JAVA_HOME')
keytool = str(pathlib.Path(java_home) / 'bin/keytool') if java_home else 'keytool'
if not key.exists():
    subprocess.run([keytool, '-genkeypair', '-keystore', str(key), '-storetype', 'PKCS12', '-alias', 'keyboard-it',
                    '-storepass:env', 'KEYBOARD_IT_ANDROID_STORE_PASSWORD', '-keypass:env', 'KEYBOARD_IT_ANDROID_KEY_PASSWORD',
                    '-keyalg', 'RSA', '-keysize', '3072', '-validity', '10000', '-dname', 'CN=keyboard-it Android,O=keyboard-it'], env=env, check=True)
    key.chmod(0o600)
subprocess.run([str(ROOT / 'packaging/android/build.sh'), 'release'], cwd=ROOT, env=env, check=True)
out = ROOT / 'dist/keyboard-it-android.apk'
out.parent.mkdir(exist_ok=True)
shutil.copy2(ROOT / 'android/app/build/outputs/apk/release/app-release.apk', out)
print('Signed APK:', out)
print('SHA-256:', hashlib.sha256(out.read_bytes()).hexdigest())
print('Keep .android-signing/ private and backed up; future APK updates require the same key.')
