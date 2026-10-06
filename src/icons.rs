use async_channel::{Receiver, Sender};
use gpui_kit::RenderImage;
use image::{Frame, RgbaImage};
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::Arc,
};

pub struct Loaded {
    pub path: PathBuf,
    pub image: Option<Arc<RenderImage>>,
}

/// Extract only visible icons on a worker; keep a bounded in-memory cache.
pub struct Icons {
    sender: Sender<PathBuf>,
    cache: HashMap<PathBuf, Option<Arc<RenderImage>>>,
    lru: VecDeque<PathBuf>,
}

impl Icons {
    pub fn new() -> (Self, Receiver<Loaded>) {
        let (sender, requests) = async_channel::bounded::<PathBuf>(64);
        let (results, receiver) = async_channel::bounded(32);
        std::thread::Builder::new()
            .name("starter-icons".into())
            .spawn(move || {
                while let Ok(path) = requests.recv_blocking() {
                    #[cfg(target_os = "macos")]
                    let image = objc2::rc::autoreleasepool(|_| load(&path));
                    #[cfg(not(target_os = "macos"))]
                    let image = load(&path);
                    if results.send_blocking(Loaded { path, image }).is_err() {
                        break;
                    }
                }
            })
            .expect("Cannot start the icon worker");
        (
            Self {
                sender,
                cache: HashMap::new(),
                lru: VecDeque::new(),
            },
            receiver,
        )
    }

    pub fn get(&mut self, path: &PathBuf) -> Option<Arc<RenderImage>> {
        if let Some(image) = self.cache.get(path) {
            // Move to back (most recently used)
            if let Some(pos) = self.lru.iter().position(|p| p == path) {
                self.lru.remove(pos);
            }
            self.lru.push_back(path.clone());
            return image.clone();
        }
        if self.sender.try_send(path.clone()).is_ok() {
            self.cache.insert(path.clone(), None);
            self.lru.push_back(path.clone());
        }
        None
    }

    pub fn insert(&mut self, loaded: Loaded) {
        const MAX_CACHE_SIZE: usize = 512;

        // Evict least recently used if at capacity
        while self.cache.len() >= MAX_CACHE_SIZE && !self.lru.is_empty() {
            if let Some(old_path) = self.lru.pop_front() {
                self.cache.remove(&old_path);
            }
        }

        self.cache.insert(loaded.path.clone(), loaded.image);

        // Add to LRU if not already present
        if !self.lru.iter().any(|p| p == &loaded.path) {
            self.lru.push_back(loaded.path);
        }
    }
}

fn load(path: &PathBuf) -> Option<Arc<RenderImage>> {
    let mut icon = file_icon_provider::get_file_icon(path, 64).ok()?;
    // GPUI's RenderImage stores BGRA; the OS adapter exposes RGBA.
    for pixel in icon.pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    let bitmap = RgbaImage::from_raw(icon.width, icon.height, icon.pixels)?;
    Some(Arc::new(RenderImage::new(vec![Frame::new(bitmap)])))
}
