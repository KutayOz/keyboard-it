//! Opaque, bounded handles keep Java lifecycle cancellation free of raw-pointer
//! ownership races. Secrets cross JNI only for encrypted Android Keystore storage.
use controller_client::{PairEvent, Pairing, Session, Target};
use jni::{
    objects::{JByteArray, JClass, JString},
    sys::{jbyteArray, jint, jlong, jstring},
    JNIEnv,
};
use std::{
    collections::HashMap,
    io, ptr,
    sync::{
        atomic::{AtomicI64, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::Duration,
};
static NEXT: AtomicI64 = AtomicI64::new(1);
static PAIRS: OnceLock<Mutex<HashMap<i64, Arc<Pairing>>>> = OnceLock::new();
static SESSIONS: OnceLock<Mutex<HashMap<i64, Arc<Session>>>> = OnceLock::new();
fn pairs() -> &'static Mutex<HashMap<i64, Arc<Pairing>>> {
    PAIRS.get_or_init(Default::default)
}
fn sessions() -> &'static Mutex<HashMap<i64, Arc<Session>>> {
    SESSIONS.get_or_init(Default::default)
}
fn text(env: &mut JNIEnv, s: &JString) -> io::Result<String> {
    env.get_string(s).map(Into::into).map_err(io::Error::other)
}
fn checked_port(v: jint) -> io::Result<u16> {
    u16::try_from(v)
        .ok()
        .filter(|p| *p != 0)
        .ok_or_else(|| io::Error::other("Invalid port"))
}
fn boundary<T: Default>(env: &mut JNIEnv, f: impl FnOnce(&mut JNIEnv) -> io::Result<T>) -> T {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(env))) {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            let _ = env.throw_new("java/io/IOException", e.to_string());
            T::default()
        }
        Err(_) => {
            let _ = env.throw_new("java/io/IOException", "Native session failed");
            T::default()
        }
    }
}
fn session(id: i64) -> io::Result<Arc<Session>> {
    sessions()
        .lock()
        .unwrap()
        .get(&id)
        .cloned()
        .ok_or_else(|| io::Error::other("Session closed"))
}
#[no_mangle]
pub extern "system" fn Java_com_keyboardit_controller_Native_identity(
    mut env: JNIEnv,
    _: JClass,
) -> jstring {
    boundary(&mut env, |e| {
        e.new_string(protocol::secure::generate_key())
            .map(|s| s.into_raw())
            .map_err(io::Error::other)
    })
}
#[no_mangle]
pub extern "system" fn Java_com_keyboardit_controller_Native_pairBegin(
    mut env: JNIEnv,
    _: JClass,
    host: JString,
    port: jint,
    desktop: jint,
    name: JString,
    identity: JString,
) -> jlong {
    boundary(&mut env, |e| {
        if pairs().lock().unwrap().len() >= 2 {
            return Err(io::Error::other("Another pairing is pending"));
        }
        let op = Pairing::start(
            &text(e, &host)?,
            checked_port(port)?,
            checked_port(desktop)?,
            &text(e, &name)?,
            &text(e, &identity)?,
        )?;
        let mut map = pairs().lock().unwrap();
        if map.len() >= 2 {
            return Err(io::Error::other("Another pairing is pending"));
        }
        let id = NEXT.fetch_add(1, Ordering::SeqCst);
        map.insert(id, Arc::new(op));
        Ok(id)
    })
}
#[no_mangle]
pub extern "system" fn Java_com_keyboardit_controller_Native_pairPoll(
    mut env: JNIEnv,
    _: JClass,
    id: jlong,
) -> jstring {
    boundary(&mut env, |e| {
        let op = pairs()
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| io::Error::other("Pairing cancelled"))?;
        let message = match op.poll(Duration::from_millis(250)) {
            Some(PairEvent::Fingerprint(code)) => {
                serde_json::json!({"type":"fingerprint","code":code})
            }
            Some(PairEvent::Paired(target)) => serde_json::json!({"type":"paired","target":target}),
            Some(PairEvent::Failed(error)) => serde_json::json!({"type":"error","message":error}),
            None => return Ok(ptr::null_mut()),
        };
        e.new_string(message.to_string())
            .map(|s| s.into_raw())
            .map_err(io::Error::other)
    })
}
#[no_mangle]
pub extern "system" fn Java_com_keyboardit_controller_Native_pairCancel(
    mut env: JNIEnv,
    _: JClass,
    id: jlong,
) {
    boundary(&mut env, |_| {
        if let Some(op) = pairs().lock().unwrap().remove(&id) {
            op.cancel();
        }
        Ok(())
    });
}
#[no_mangle]
pub extern "system" fn Java_com_keyboardit_controller_Native_connect(
    mut env: JNIEnv,
    _: JClass,
    target: JString,
    identity: JString,
) -> jlong {
    boundary(&mut env, |e| {
        let encoded = text(e, &target)?;
        if encoded.len() > 8192 {
            return Err(io::Error::other("Target record too large"));
        }
        let target: Target = serde_json::from_str(&encoded)
            .map_err(|_| io::Error::other("Invalid target record"))?;
        if sessions().lock().unwrap().len() >= 2 {
            return Err(io::Error::other("Close the previous session first"));
        }
        let s = Session::connect(&target, &text(e, &identity)?)?;
        let mut map = sessions().lock().unwrap();
        if map.len() >= 2 {
            return Err(io::Error::other("Close the previous session first"));
        }
        let id = NEXT.fetch_add(1, Ordering::SeqCst);
        map.insert(id, Arc::new(s));
        Ok(id)
    })
}
#[no_mangle]
pub extern "system" fn Java_com_keyboardit_controller_Native_poll(
    mut env: JNIEnv,
    _: JClass,
    id: jlong,
) -> jbyteArray {
    boundary(&mut env, |e| {
        match session(id)?.poll(Duration::from_millis(250)) {
            Some(event) => e
                .byte_array_from_slice(&event.packet())
                .map(|a| a.into_raw())
                .map_err(io::Error::other),
            None => Ok(ptr::null_mut()),
        }
    })
}
#[no_mangle]
pub extern "system" fn Java_com_keyboardit_controller_Native_input(
    mut env: JNIEnv,
    _: JClass,
    id: jlong,
    input: JByteArray,
) {
    boundary(&mut env, |e| {
        if e.get_array_length(&input).map_err(io::Error::other)? as usize > protocol::INPUT_MAX_LEN
        {
            return Err(io::Error::other("Input too large"));
        }
        let bytes = e.convert_byte_array(&input).map_err(io::Error::other)?;
        let event =
            protocol::InputEvent::decode(&bytes).map_err(|_| io::Error::other("Invalid input"))?;
        session(id)?.input(event)
    });
}
#[no_mangle]
pub extern "system" fn Java_com_keyboardit_controller_Native_absolute(
    mut env: JNIEnv,
    _: JClass,
    id: jlong,
    x: jint,
    y: jint,
) {
    boundary(&mut env, |_| {
        let x = u16::try_from(x).map_err(io::Error::other)?;
        let y = u16::try_from(y).map_err(io::Error::other)?;
        session(id)?.absolute(x, y)
    })
}
#[no_mangle]
pub extern "system" fn Java_com_keyboardit_controller_Native_release(
    mut env: JNIEnv,
    _: JClass,
    id: jlong,
) {
    boundary(&mut env, |_| session(id)?.release());
}
#[no_mangle]
pub extern "system" fn Java_com_keyboardit_controller_Native_close(
    mut env: JNIEnv,
    _: JClass,
    id: jlong,
) {
    boundary(&mut env, |_| {
        if let Some(s) = sessions().lock().unwrap().remove(&id) {
            s.close();
        }
        Ok(())
    });
}

#[no_mangle]
pub extern "system" fn Java_com_keyboardit_controller_Native_text(
    mut env: JNIEnv,
    _: JClass,
    id: jlong,
    input: JString,
) {
    boundary(&mut env, |e| session(id)?.text(&text(e, &input)?));
}
