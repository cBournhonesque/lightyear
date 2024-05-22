//! Module containing extra behaviour that we need when running in wasm

use bevy::prelude::*;
use std::sync::{Arc, RwLock};
use wasm_bindgen::{closure::Closure, JsCast, JsValue};
use web_sys::{js_sys::Array, window, Blob, Url, Worker};

pub struct WebPlugin;

impl Plugin for WebPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_background_worker);
    }
}

/// In wasm, the main thread gets quickly throttled by the browser when it is hidden (e.g. when the user switches tabs).
/// This means that the app.update() function will not be called, because bevy's scheduler only runs `app.update()` when
/// the browser's requestAnimationFrame is called. (and that happens only when the tab is visible)
///
/// This is problematic because:
/// - we stop sending packets so the server disconnects the client because it doesn't receive keep-alives
/// - when the client reconnects, it also disconnects because it hasn't been receiving server packets
/// - the internal transport buffers can overflow because they are not being emptied
///
/// This solution spawns a WebWorker (a background thread) which is not throttled, and which runs
/// `app.update()` at a fixed interval. This way, the client can keep sending and receiving packets,
/// and updating the local World.
fn spawn_background_worker(world: &mut World) {
    let world_ptr = Arc::new(RwLock::new(world as *mut World));

    // The interval is in milliseconds. We can run app.update() infrequently when in the background
    let blob = Blob::new_with_str_sequence(
        &Array::of1(&JsValue::from_str(
            "setInterval(() => self.postMessage(0), 1000);",
        ))
        .unchecked_into(),
    )
    .unwrap();

    let worker = Worker::new(&Url::create_object_url_with_blob(&blob).unwrap()).unwrap();

    let closure = Closure::<dyn FnMut()>::new(move || {
        if window().unwrap().document().unwrap().hidden() {
            // Imitate app.update()
            let world = unsafe { world_ptr.write().unwrap().as_mut().unwrap() };
            world.run_schedule(Main);
            world.clear_trackers();
        }
    });

    worker.set_onmessage(Some(closure.as_ref().unchecked_ref()));

    closure.forget();
}
