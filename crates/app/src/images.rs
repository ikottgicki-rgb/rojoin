//! Lazy thumbnail loading.
//!
//! This is where v2's memory went. Eagerly fetching and holding every image a
//! screen might show is how a Roblox client ends up at 870 MB, so:
//!   * fetch and decode happen off the UI thread,
//!   * the shared cache holds plain RGBA bytes (which are `Send`) under a
//!     bounded LRU, not `slint::Image` (which is not),
//!   * `slint::Image` values are built on the UI thread and memoised there.
//!
//! **The apply is always deferred, even on a cache hit.** Calling
//! `model.set_row_data()` synchronously from a repeater delegate's `init`
//! re-enters the model and panics with "RefCell already borrowed". Routing
//! every apply through `upgrade_in_event_loop` makes that structurally
//! impossible rather than a rule someone has to remember.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

use slint::ComponentHandle;

use crate::MainWindow;

/// A callback waiting on an in-flight image.
type Waiter = Box<dyn FnOnce(MainWindow, Image) + Send>;

/// Decoded pixels, cheap to share between threads.
pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Roughly 400 thumbnails. At 256x144 RGBA that is about 59 MB worst case,
/// and real thumbnails are smaller.
const CAPACITY: usize = 400;

#[derive(Default)]
struct Cache {
    map: HashMap<String, Arc<Decoded>>,
    /// Insertion order, for eviction. Small enough that a Vec beats a real
    /// LRU structure.
    order: Vec<String>,
}

impl Cache {
    fn get(&self, url: &str) -> Option<Arc<Decoded>> {
        self.map.get(url).cloned()
    }

    fn insert(&mut self, url: String, decoded: Arc<Decoded>) {
        if self.map.insert(url.clone(), decoded).is_none() {
            self.order.push(url);
        }
        while self.order.len() > CAPACITY {
            let oldest = self.order.remove(0);
            self.map.remove(&oldest);
        }
    }
}

thread_local! {
    /// UI-thread memo so a repeated cache hit does not rebuild the pixel
    /// buffer on every rebind of a virtualised row.
    static UI_IMAGES: RefCell<HashMap<String, Image>> = RefCell::new(HashMap::new());
}

#[derive(Clone)]
pub struct Images {
    client: rojoin_roblox::Client,
    cache: Arc<Mutex<Cache>>,
    /// URLs being fetched, each with the callbacks waiting on it.
    ///
    /// Waiters are QUEUED, not dropped. Dropping the second request for a URL
    /// meant the same image used in two places only appeared in one — the
    /// Home hero stayed blank because the Recent grid had already asked for
    /// that exact thumbnail.
    inflight: Arc<Mutex<HashMap<String, Vec<Waiter>>>>,
    ui: slint::Weak<MainWindow>,
    rt: Arc<tokio::runtime::Runtime>,
}

impl Images {
    pub fn new(
        client: rojoin_roblox::Client,
        ui: slint::Weak<MainWindow>,
        rt: Arc<tokio::runtime::Runtime>,
    ) -> Self {
        Self {
            client,
            cache: Arc::new(Mutex::new(Cache::default())),
            inflight: Arc::new(Mutex::new(Default::default())),
            ui,
            rt,
        }
    }

    /// Request `url`; `apply` runs on the UI thread once pixels are available.
    ///
    /// Always asynchronous from the caller's point of view — see the module
    /// docs for why a synchronous cache-hit path would be a latent panic.
    pub fn load<F>(&self, url: &str, apply: F)
    where
        F: FnOnce(MainWindow, Image) + Send + 'static,
    {
        if url.is_empty() {
            return;
        }

        let url = url.to_string();

        if let Some(decoded) = self.cache.lock().ok().and_then(|c| c.get(&url)) {
            let ui = self.ui.clone();
            let key = url.clone();
            let _ = ui.upgrade_in_event_loop(move |handle| {
                apply(handle, to_image(&key, &decoded));
            });
            return;
        }

        {
            let mut inflight = match self.inflight.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            if let Some(waiters) = inflight.get_mut(&url) {
                waiters.push(Box::new(apply));
                return;
            }
            inflight.insert(url.clone(), vec![Box::new(apply)]);
        }

        let client = self.client.clone();
        let cache = self.cache.clone();
        let inflight = self.inflight.clone();
        let ui = self.ui.clone();

        self.rt.spawn(async move {
            let decoded = match client.fetch_bytes(&url).await {
                Ok(bytes) => tokio::task::spawn_blocking(move || decode(&bytes))
                    .await
                    .ok()
                    .flatten(),
                Err(e) => {
                    tracing::debug!(url, error = %e, "thumbnail fetch failed");
                    None
                }
            };

            let waiters = inflight
                .lock()
                .ok()
                .and_then(|mut g| g.remove(&url))
                .unwrap_or_default();

            let Some(decoded) = decoded else { return };
            let decoded = Arc::new(decoded);

            if let Ok(mut c) = cache.lock() {
                c.insert(url.clone(), decoded.clone());
            }

            let _ = ui.upgrade_in_event_loop(move |handle| {
                let img = to_image(&url, &decoded);
                for waiter in waiters {
                    waiter(handle.clone_strong(), img.clone());
                }
            });
        });
    }

    /// Drop everything. Called on account switch so one account's avatars
    /// never flash up under another's name.
    pub fn clear(&self) {
        if let Ok(mut c) = self.cache.lock() {
            c.map.clear();
            c.order.clear();
        }
        UI_IMAGES.with(|m| m.borrow_mut().clear());
    }
}

fn decode(bytes: &[u8]) -> Option<Decoded> {
    let img = image::load_from_memory(bytes).ok()?.into_rgba8();
    Some(Decoded {
        width: img.width(),
        height: img.height(),
        rgba: img.into_raw(),
    })
}

/// Build (or reuse) a `slint::Image`. UI thread only.
fn to_image(url: &str, decoded: &Decoded) -> Image {
    UI_IMAGES.with(|memo| {
        let mut memo = memo.borrow_mut();
        if let Some(img) = memo.get(url) {
            return img.clone();
        }

        let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(decoded.width, decoded.height);
        buf.make_mut_bytes().copy_from_slice(&decoded.rgba);
        let img = Image::from_rgba8(buf);

        if memo.len() > CAPACITY {
            memo.clear();
        }
        memo.insert(url.to_string(), img.clone());
        img
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_evicts_oldest_beyond_capacity() {
        let mut c = Cache::default();
        for i in 0..(CAPACITY + 10) {
            c.insert(
                format!("u{i}"),
                Arc::new(Decoded { width: 1, height: 1, rgba: vec![0; 4] }),
            );
        }

        assert_eq!(c.map.len(), CAPACITY);
        assert!(c.get("u0").is_none(), "oldest entry should have been evicted");
        assert!(c.get(&format!("u{}", CAPACITY + 9)).is_some());
    }

    #[test]
    fn reinserting_the_same_url_does_not_grow_the_order_list() {
        let mut c = Cache::default();
        for _ in 0..5 {
            c.insert(
                "same".into(),
                Arc::new(Decoded { width: 1, height: 1, rgba: vec![0; 4] }),
            );
        }
        assert_eq!(c.order.len(), 1);
        assert_eq!(c.map.len(), 1);
    }

    #[test]
    fn waiters_queue_rather_than_replace() {
        let mut inflight: HashMap<String, Vec<u32>> = HashMap::new();
        inflight.entry("u".into()).or_default().push(1);
        inflight.entry("u".into()).or_default().push(2);
        assert_eq!(inflight["u"], vec![1, 2]);
    }

    #[test]
    fn decoding_garbage_yields_none_rather_than_panicking() {
        assert!(decode(b"definitely not a png").is_none());
        assert!(decode(&[]).is_none());
    }
}
