use async_channel::{Receiver, Sender};
use gpui_kit::RenderImage;
use image::{Frame, RgbaImage};
use std::{collections::HashMap, path::PathBuf, sync::Arc};

pub struct Loaded {
    pub path: PathBuf,
    pub image: Option<Arc<RenderImage>>,
}

/// Extract only visible icons on a worker; keep a bounded in-memory cache.
pub struct Icons {
    sender: Sender<PathBuf>,
    cache: HashMap<PathBuf, Option<Arc<RenderImage>>>,
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
            },
            receiver,
        )
    }

    pub fn get(&mut self, path: &PathBuf) -> Option<Arc<RenderImage>> {
        if let Some(image) = self.cache.get(path) {
            return image.clone();
        }
        if self.sender.try_send(path.clone()).is_ok() {
            self.cache.insert(path.clone(), None);
        }
        None
    }

    pub fn insert(&mut self, loaded: Loaded) {
        if self.cache.len() >= 512 {
            self.cache.clear();
        }
        self.cache.insert(loaded.path, loaded.image);
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
