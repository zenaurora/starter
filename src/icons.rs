use async_channel::{Receiver, Sender};
use gpui_kit::RenderImage;
use image::{Frame, RgbaImage};
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    path::PathBuf,
    rc::Rc,
    sync::Arc,
};

pub struct Loaded {
    pub path: PathBuf,
    pub image: Option<Arc<RenderImage>>,
}

/// Extract only visible icons on a worker; keep a bounded in-memory cache.
/// Cloned handles share both loaded images and requests already in flight.
#[derive(Clone)]
pub struct Icons {
    sender: Sender<PathBuf>,
    cache: Rc<RefCell<Cache>>,
}

#[derive(Default)]
struct Cache {
    images: HashMap<PathBuf, Option<Arc<RenderImage>>>,
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
                cache: Rc::default(),
            },
            receiver,
        )
    }

    pub fn get(&mut self, path: &PathBuf) -> Option<Arc<RenderImage>> {
        let Cache { images, lru } = &mut *self.cache.borrow_mut();
        if let Some(image) = images.get(path) {
            // Move to back (most recently used)
            if let Some(pos) = lru.iter().position(|p| p == path) {
                lru.remove(pos);
            }
            lru.push_back(path.clone());
            return image.clone();
        }
        if self.sender.try_send(path.clone()).is_ok() {
            images.insert(path.clone(), None);
            lru.push_back(path.clone());
        }
        None
    }

    pub fn insert(&mut self, loaded: Loaded) {
        const MAX_CACHE_SIZE: usize = 512;
        let Cache { images, lru } = &mut *self.cache.borrow_mut();

        // Evict least recently used if at capacity
        while images.len() >= MAX_CACHE_SIZE && !lru.is_empty() {
            if let Some(old_path) = lru.pop_front() {
                images.remove(&old_path);
            }
        }

        images.insert(loaded.path.clone(), loaded.image);

        // Add to LRU if not already present
        if !lru.iter().any(|p| p == &loaded.path) {
            lru.push_back(loaded.path);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_handles_reuse_in_flight_requests_loaded_images_and_failed_results() {
        let (sender, requests) = async_channel::bounded(4);
        let mut launcher = Icons {
            sender,
            cache: Rc::default(),
        };
        let mut panel = launcher.clone();
        let path = PathBuf::from("/Applications/Test.app");
        assert!(launcher.get(&path).is_none());
        assert!(panel.get(&path).is_none());
        assert_eq!(requests.try_recv().unwrap(), path);
        assert!(requests.is_empty());

        let image = Arc::new(RenderImage::new(vec![Frame::new(RgbaImage::new(1, 1))]));
        launcher.insert(Loaded {
            path: path.clone(),
            image: Some(image.clone()),
        });
        assert!(Arc::ptr_eq(&panel.get(&path).unwrap(), &image));
        assert!(requests.is_empty());

        launcher.insert(Loaded {
            path: path.clone(),
            image: None,
        });
        assert!(panel.get(&path).is_none());
        assert!(requests.is_empty());
    }
}
