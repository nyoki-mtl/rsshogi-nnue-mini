//! Opaque C handles. All normal operations serialize; stop only touches a separate atomic token.

use std::ffi::{CStr, CString, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Mutex;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::api::{Engine, SearchOptions, StopHandle};

const MAX_BUFFER: usize = 64 * 1024 * 1024;

pub struct MiniEngine {
    engine: Mutex<Engine>,
    stop: StopHandle,
}

#[derive(Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    LoadEmbedded,
    SetPosition {
        sfen: String,
        #[serde(default)]
        moves: Vec<String>,
    },
    Configure {
        hash_mb: Option<usize>,
        threads: Option<usize>,
    },
    NewGame,
    Evaluate,
    Search {
        depth: Option<u32>,
        nodes: Option<u64>,
        movetime_ms: Option<u64>,
    },
}

fn response(action: impl FnOnce() -> Result<Value, String>) -> *mut c_char {
    let value = match catch_unwind(AssertUnwindSafe(action)) {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => json!({"ok": false, "error": error}),
        Err(_) => {
            json!({"ok": false, "error": "engine panicked; destroy and recreate this handle"})
        }
    };
    CString::new(value.to_string()).expect("JSON strings escape NUL").into_raw()
}

#[unsafe(no_mangle)]
pub extern "C" fn mini_engine_new() -> *mut MiniEngine {
    catch_unwind(|| {
        let engine = Engine::new();
        let stop = engine.stop_handle();
        Box::into_raw(Box::new(MiniEngine { engine: Mutex::new(engine), stop }))
    })
    .unwrap_or(std::ptr::null_mut())
}

/// # Safety
/// The pointer must be a live handle from mini_engine_new. No operation, including stop,
/// may be active or begin concurrently with this call. Free each handle exactly once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mini_engine_free(handle: *mut MiniEngine) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle) });
    }
}

/// # Safety
/// A non-null handle must be live for the duration of this call, including concurrent stop.
/// An idle stop is sticky and is consumed by the next search.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mini_engine_stop(handle: *mut MiniEngine) {
    if let Some(handle) = unsafe { handle.as_ref() } {
        handle.stop.stop();
    }
}

/// # Safety
/// The handle must be live and bytes must reference len readable bytes. The memory is
/// borrowed only for this call. The returned JSON must be freed with mini_string_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mini_engine_load_model(
    handle: *mut MiniEngine,
    bytes: *const u8,
    len: usize,
) -> *mut c_char {
    response(|| {
        let handle = unsafe { handle.as_ref() }.ok_or("null engine handle")?;
        if bytes.is_null() || len == 0 || len > MAX_BUFFER {
            return Err("model buffer must contain 1..=67108864 bytes".to_owned());
        }
        let bytes = unsafe { std::slice::from_raw_parts(bytes, len) };
        let mut engine =
            handle.engine.lock().map_err(|_| "engine lock poisoned; recreate handle")?;
        let digest = engine.load_model(bytes)?;
        Ok(json!({"ok": true, "digest": digest}))
    })
}

/// # Safety
/// The handle must be live and request must be a NUL-terminated UTF-8 string. Request
/// methods may serialize concurrently, but free must not run until all calls return.
/// The returned JSON belongs to the caller and must be freed with mini_string_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mini_engine_request(
    handle: *mut MiniEngine,
    request: *const c_char,
) -> *mut c_char {
    response(|| {
        let handle = unsafe { handle.as_ref() }.ok_or("null engine handle")?;
        if request.is_null() {
            return Err("null request".to_owned());
        }
        let request = unsafe { CStr::from_ptr(request) }.to_str().map_err(|e| e.to_string())?;
        let request: Request = serde_json::from_str(request).map_err(|e| e.to_string())?;
        let mut engine =
            handle.engine.lock().map_err(|_| "engine lock poisoned; recreate handle")?;
        dispatch(&mut engine, request)
    })
}

fn dispatch(engine: &mut Engine, request: Request) -> Result<Value, String> {
    match request {
        Request::LoadEmbedded => Ok(json!({"ok": true, "digest": engine.load_embedded()?})),
        Request::SetPosition { sfen, moves } => {
            engine.set_position(&sfen, &moves)?;
            Ok(json!({"ok": true}))
        }
        Request::Configure { hash_mb, threads } => {
            engine.configure(hash_mb, threads)?;
            Ok(json!({"ok": true}))
        }
        Request::NewGame => {
            engine.new_game();
            Ok(json!({"ok": true}))
        }
        Request::Evaluate => Ok(json!({"ok": true, "score": engine.evaluate()?})),
        Request::Search { depth, nodes, movetime_ms } => {
            if depth.is_none() && nodes.is_none() && movetime_ms.is_none() {
                return Err("search requires depth, nodes or movetime_ms".to_owned());
            }
            let result =
                engine.search(SearchOptions { depth: depth.unwrap_or(64), nodes, movetime_ms })?;
            Ok(json!({"ok": true, "result": result}))
        }
    }
}

/// # Safety
/// Only pass a non-null pointer returned by mini_engine_request or mini_engine_load_model,
/// once, without modifying it or using it after this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mini_string_free(value: *mut c_char) {
    if !value.is_null() {
        drop(unsafe { CString::from_raw(value) });
    }
}

/// Zeroed host transfer buffer, primarily for JavaScript linear-memory transfers.
#[unsafe(no_mangle)]
pub extern "C" fn mini_alloc(len: usize) -> *mut u8 {
    if len == 0 || len > MAX_BUFFER {
        return std::ptr::null_mut();
    }
    Box::into_raw(vec![0_u8; len].into_boxed_slice()).cast::<u8>()
}

/// # Safety
/// ptr must be returned by mini_alloc(len), with exactly that len, and freed exactly once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mini_dealloc(ptr: *mut u8, len: usize) {
    if !ptr.is_null() {
        drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe fn request(handle: *mut MiniEngine, text: &str) -> Value {
        let text = CString::new(text).unwrap();
        let output = unsafe { mini_engine_request(handle, text.as_ptr()) };
        let value =
            serde_json::from_str(unsafe { CStr::from_ptr(output) }.to_str().unwrap()).unwrap();
        unsafe { mini_string_free(output) };
        value
    }

    #[test]
    fn c_boundary_checks_errors_and_retains_engine_state() {
        let engine = Engine::fixture();
        let stop = engine.stop_handle();
        let handle = Box::into_raw(Box::new(MiniEngine { engine: Mutex::new(engine), stop }));
        unsafe {
            assert_eq!(request(handle, r#"{"command":"search","depth":2}"#)["result"]["depth"], 2);
            assert_eq!(request(handle, r#"{"command":"search"}"#)["ok"], false);
            assert_eq!(request(handle, r#"{"command":"search","nodes":0}"#)["ok"], false);
            assert_eq!(request(handle, r#"{"command":"search","depht":2}"#)["ok"], false);
            assert_eq!(
                request(handle, r#"{"command":"set_position","sfen":"startpos","moves":["7g7e"]}"#)
                    ["ok"],
                false
            );
            mini_engine_stop(handle);
            assert_eq!(request(handle, r#"{"command":"search","depth":2}"#)["result"]["depth"], 0);
            assert_eq!(request(handle, r#"{"command":"search","depth":2}"#)["result"]["depth"], 2);
            assert_eq!(request(std::ptr::null_mut(), "{}")["ok"], false);
            mini_engine_free(handle);
        }
    }
}
