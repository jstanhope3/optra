//! Decoding images off the UI thread, and keeping neighbours ready.
//!
//! Scrubbing through a sequence is only smooth if the next frame is already in
//! RAM: a 4K EXR is ~130 MB of float data and takes far longer than a frame to
//! decode. So neighbours are decoded on background threads and parked here.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use eframe::wgpu;

/// An image decoded and laid out exactly as the GPU texture wants it.
pub struct DecodedImage {
    pub size: [u32; 2],
    /// Raw texel bytes: 8-bit RGBA, or 32-bit float RGBA for HDR sources.
    pub bytes: Vec<u8>,
    pub is_hdr: bool,
}

impl DecodedImage {
    pub fn texture_format(&self) -> wgpu::TextureFormat {
        if self.is_hdr {
            wgpu::TextureFormat::Rgba32Float
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        }
    }
}

/// Decode a file from disk. Safe to call from any thread.
pub fn decode(path: &Path) -> Option<DecodedImage> {
    match image::open(path) {
        Ok(dynamic) => Some(from_dynamic(dynamic)),
        Err(e) => {
            eprintln!("Could not open {path:?}: {e}");
            None
        }
    }
}

/// Decode an image already in memory -- how remote files arrive, since SFTP
/// hands us bytes rather than a path we could open.
pub fn decode_bytes(bytes: &[u8], label: &Path) -> Option<DecodedImage> {
    match image::load_from_memory(bytes) {
        Ok(dynamic) => Some(from_dynamic(dynamic)),
        Err(e) => {
            eprintln!("Could not decode {label:?}: {e}");
            None
        }
    }
}

fn from_dynamic(dynamic: image::DynamicImage) -> DecodedImage {
    // EXR and Radiance HDR decode to linear float; keep the full range rather
    // than clipping it into 8 bits.
    let is_hdr = matches!(
        dynamic,
        image::DynamicImage::ImageRgb32F(_) | image::DynamicImage::ImageRgba32F(_)
    );

    let size = [dynamic.width(), dynamic.height()];

    let bytes = if is_hdr {
        let float_img = dynamic.to_rgba32f();
        bytemuck::cast_slice::<f32, u8>(float_img.as_raw()).to_vec()
    } else {
        dynamic.to_rgba8().into_raw()
    };

    DecodedImage {
        size,
        bytes,
        is_hdr,
    }
}

/// Decoded images held in RAM, plus the set currently being decoded.
#[derive(Clone, Default)]
pub struct ImageCache {
    entries: Arc<Mutex<HashMap<PathBuf, Arc<DecodedImage>>>>,
    in_flight: Arc<Mutex<HashSet<PathBuf>>>,
}

impl ImageCache {
    pub fn get(&self, path: &Path) -> Option<Arc<DecodedImage>> {
        self.entries.lock().unwrap().get(path).cloned()
    }

    pub fn insert(&self, path: PathBuf, image: Arc<DecodedImage>) {
        self.entries.lock().unwrap().insert(path, image);
    }

    /// Decode `path` on a background thread, unless it is already cached or
    /// being decoded. Returns immediately.
    pub fn request(&self, path: PathBuf) {
        if self.entries.lock().unwrap().contains_key(&path) {
            return;
        }
        if !self.in_flight.lock().unwrap().insert(path.clone()) {
            return; // already being decoded
        }

        let entries = Arc::clone(&self.entries);
        let in_flight = Arc::clone(&self.in_flight);

        std::thread::spawn(move || {
            let decoded = decode(&path);
            if let Some(decoded) = decoded {
                entries
                    .lock()
                    .unwrap()
                    .insert(path.clone(), Arc::new(decoded));
            }
            in_flight.lock().unwrap().remove(&path);
        });
    }

    /// Drop anything outside `keep`. Without this, scrubbing a long sequence
    /// would hold every frame ever visited in RAM.
    pub fn retain(&self, keep: &HashSet<PathBuf>) {
        self.entries.lock().unwrap().retain(|k, _| keep.contains(k));
    }

    /// Number of images currently held, for the settings display.
    pub fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
