use crate::image_cache::{self, ImageCache};
use crate::open_file;
use crate::remote::{FileState, ListState, RemoteFs, Status};
use crate::render::gpuimage::GpuImage;
use crate::render::wgpuimage::DataBlock;
use eframe::wgpu;
use egui::Frame;
use egui::Margin;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc as StdArc;

/// Extensions listed in the file tree. Directories are always shown.
const IMAGE_EXTENSIONS: [&str; 12] = [
    "png", "jpg", "jpeg", "gif", "bmp", "tif", "tiff", "webp", "tga", "qoi", "exr", "hdr",
];

/// Catppuccin flavours, in the order the upstream palette lists them.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeChoice {
    Latte,
    Frappe,
    #[default]
    Macchiato,
    Mocha,
}

impl ThemeChoice {
    const ALL: [Self; 4] = [Self::Latte, Self::Frappe, Self::Macchiato, Self::Mocha];

    fn label(self) -> &'static str {
        match self {
            Self::Latte => "Latte",
            Self::Frappe => "Frappe",
            Self::Macchiato => "Macchiato",
            Self::Mocha => "Mocha",
        }
    }

    fn theme(self) -> catppuccin_egui::Theme {
        match self {
            Self::Latte => catppuccin_egui::LATTE,
            Self::Frappe => catppuccin_egui::FRAPPE,
            Self::Macchiato => catppuccin_egui::MACCHIATO,
            Self::Mocha => catppuccin_egui::MOCHA,
        }
    }

    /// The two checkerboard squares shown behind a letterboxed image.
    /// `mantle`/`crust` sit just either side of the panel background, so the
    /// checker reads as part of the theme rather than a grey hole in it.
    fn checker_colors(self) -> ([f32; 3], [f32; 3]) {
        let t = self.theme();
        (srgb_components(t.mantle), srgb_components(t.crust))
    }
}

/// egui colours are 8-bit sRGB. The render target is `Bgra8Unorm` (not an sRGB
/// format), so values are displayed exactly as written -- same as the 8-bit
/// image path -- and need no linearisation here.
fn srgb_components(c: egui::Color32) -> [f32; 3] {
    [
        c.r() as f32 / 255.0,
        c.g() as f32 / 255.0,
        c.b() as f32 / 255.0,
    ]
}

pub struct App {
    image_display: Option<GpuImage>,
    app_state: AppState,
    /// Directory the file tree is rooted at.
    tree_root: PathBuf,
    /// Decoded images kept in RAM for smooth scrubbing.
    cache: ImageCache,
    /// Direction of the in-progress scrub, or 0. Used to detect a fresh press.
    scrub_dir: isize,
    /// `Context::input().time` at the last scrub step.
    last_scrub_time: f64,
    /// Theme currently applied to the egui context, to avoid restyling every frame.
    applied_theme: Option<ThemeChoice>,
    /// Whether the settings window is showing.
    settings_open: bool,
    /// Active remote connection. While set, the tree and loader browse the
    /// remote machine instead of this one.
    remote: Option<RemoteFs>,
    /// Whether the connect dialog is showing.
    remote_dialog: bool,
    remote_host: String,
    remote_user: String,
    remote_port: String,
    /// Local tree root, restored on disconnect.
    local_root: PathBuf,
    /// A remote file we are waiting on bytes for.
    pending_remote: Option<PathBuf>,
    /// Whether we have already jumped to the remote home directory.
    remote_ready: bool,
    /// Window title to apply on the next frame.
    pending_title: Option<String>,
}

#[allow(unused)]
#[derive(Default)]
struct AppState {
    current_img_idx: usize,
    file_path: Option<PathBuf>,
    display_image_size: [u32; 2],
    image_container_size_px: [u32; 2],
    /// View zoom factor (1.0 = image fitted to the container).
    zoom: f32,
    /// View offset in container-uv units (fraction of the container size).
    pan: [f32; 2],
    /// True when the loaded image is linear float data (EXR/HDR).
    is_hdr: bool,
    /// Exposure in stops, applied to HDR images only.
    exposure: f32,
    /// Display gamma, applied to HDR images only.
    gamma: f32,
    /// Sequence key of the loaded image; see [`sequence_key`].
    last_sequence_key: Option<String>,
    /// Files in the current directory sharing the loaded image's sequence key,
    /// in numeric order. Arrow keys step through this.
    sequence: Vec<PathBuf>,
    /// Position of the loaded image within [`AppState::sequence`].
    sequence_index: usize,
    /// Decode neighbouring frames ahead of time.
    preload_enabled: bool,
    /// How many neighbours either side to keep decoded.
    preload_radius: usize,
    /// Frames per second when an arrow key is held down.
    scrub_fps: f32,
    /// Active Catppuccin flavour.
    theme: ThemeChoice,
}

impl App {
    pub fn new<'a>(cc: &'a eframe::CreationContext<'a>) -> Option<Self> {
        let state = cc
            .wgpu_render_state
            .as_ref()
            .expect("Must use wgpu to render UI.");

        let display_image_size = [0, 0];

        let app_state = AppState {
            current_img_idx: 0,
            file_path: None,
            display_image_size,
            zoom: 1.0,
            gamma: 2.2,
            preload_enabled: true,
            preload_radius: 2,
            scrub_fps: 24.0,
            ..Default::default()
        };

        let device = &state.device;
        let queue = &state.queue;

        let default_img_size = [1024, 1024];

        let image_display: Option<GpuImage> = GpuImage::new(
            String::from("DisplayedImage"),
            default_img_size,
            state,
            device,
            queue,
            wgpu::TextureFormat::Rgba8Unorm,
        );

        // Lets a file arriving while the window is idle wake the event loop.
        open_file::set_repaint_context(cc.egui_ctx.clone());

        let tree_root = std::env::home_dir()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("/"));

        Some(Self {
            app_state,
            image_display,
            tree_root: tree_root.clone(),
            cache: ImageCache::default(),
            scrub_dir: 0,
            last_scrub_time: 0.0,
            applied_theme: None,
            settings_open: false,
            remote: None,
            remote_dialog: false,
            remote_host: String::new(),
            remote_user: std::env::var("USER").unwrap_or_default(),
            remote_port: "22".to_string(),
            local_root: tree_root.clone(),
            pending_remote: None,
            remote_ready: false,
            pending_title: None,
        })
    }

    /// Decode the frames around the current one, and drop the rest.
    fn preload_neighbours(&self) {
        let state = &self.app_state;

        if state.sequence.is_empty() {
            return;
        }

        // With preloading off the window collapses to the current frame, so the
        // `retain` below also frees whatever was held.
        let radius = if state.preload_enabled {
            state.preload_radius as isize
        } else {
            0
        };
        let current = state.sequence_index as isize;

        let window: Vec<PathBuf> = (-radius..=radius)
            .map(|offset| current + offset)
            .filter(|i| *i >= 0 && (*i as usize) < state.sequence.len())
            .map(|i| state.sequence[i as usize].clone())
            .collect();

        // Bound memory first, then queue the decodes.
        self.cache
            .retain(&window.iter().cloned().collect::<HashSet<_>>());

        if !state.preload_enabled {
            return;
        }

        for path in window {
            match &self.remote {
                // The SFTP worker fetches serially in the background; by the
                // time we step, the bytes are already local and only the decode
                // remains.
                Some(remote) => {
                    remote.file(&path);
                }
                None => self.cache.request(path),
            }
        }
    }

    /// Arrow keys scrub the sequence: a tap moves one frame, holding advances at
    /// `scrub_fps` regardless of the OS key-repeat rate.
    fn handle_scrub_keys(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // Don't steal arrow keys from a focused slider.
        if ctx.wants_keyboard_input() {
            self.scrub_dir = 0;
            return;
        }

        let (dir, now) = ctx.input(|i| {
            (
                i.key_down(egui::Key::ArrowRight) as isize
                    - i.key_down(egui::Key::ArrowLeft) as isize,
                i.time,
            )
        });

        if dir == 0 {
            self.scrub_dir = 0;
            return;
        }

        let interval = 1.0 / self.app_state.scrub_fps.max(1.0) as f64;

        if self.scrub_dir != dir {
            // Fresh press, or a reversal: step at once so a tap feels instant.
            self.scrub_dir = dir;
            self.last_scrub_time = now;
            self.step_sequence(dir, frame);
        } else if now - self.last_scrub_time >= interval {
            // Advance the clock by whole intervals rather than snapping it to
            // `now`. Each step lands on a frame boundary, so assigning `now`
            // would round the period up every time and run slow.
            self.last_scrub_time += interval;

            // If we fell badly behind (a slow decode, a stall), resynchronise
            // instead of firing a burst of catch-up steps.
            if now - self.last_scrub_time >= interval {
                self.last_scrub_time = now;
            }

            self.step_sequence(dir, frame);
        }

        // Render continuously while held: `request_repaint_after` would wake us
        // `interval` after this frame *ends*, making the real period
        // `frame_time + interval`. Our own clock above caps the actual rate.
        ctx.request_repaint();
    }

    /// Move `delta` frames through the current sequence.
    fn step_sequence(&mut self, delta: isize, frame: &mut eframe::Frame) {
        if self.app_state.sequence.len() < 2 {
            return;
        }

        let last = self.app_state.sequence.len() as isize - 1;
        let target = (self.app_state.sequence_index as isize + delta).clamp(0, last) as usize;

        if target == self.app_state.sequence_index {
            return; // already at an end
        }

        let path = self.app_state.sequence[target].clone();
        self.load_image(&path, frame);
    }

    fn load_image(&mut self, path: &Path, frame: &mut eframe::Frame) {
        self.app_state.file_path = Some(path.to_path_buf());

        println!("Opening image at {:?}", self.app_state.file_path);

        // Prefer an already-decoded copy from the preloader; otherwise decode
        // now and keep it, so stepping back is instant too.
        //
        // Remote files cannot be decoded inline: the bytes arrive over the
        // network on the worker thread, so the first attempt records the file
        // as pending and the next frame retries.
        enum Fetch {
            Ready(StdArc<image_cache::DecodedImage>),
            Pending,
            Failed,
        }

        let outcome = if let Some(cached) = self.cache.get(path) {
            Fetch::Ready(cached)
        } else if let Some(remote) = &self.remote {
            match remote.file(path) {
                FileState::Loading => Fetch::Pending,
                FileState::Failed(e) => {
                    eprintln!("Could not fetch {path:?}: {e}");
                    Fetch::Failed
                }
                FileState::Ready(bytes) => match image_cache::decode_bytes(&bytes, path) {
                    Some(decoded) => {
                        let decoded = StdArc::new(decoded);
                        self.cache
                            .insert(path.to_path_buf(), StdArc::clone(&decoded));
                        // The decoded copy supersedes the raw bytes; keeping
                        // both would double the memory for every frame.
                        remote.forget_file(path);
                        Fetch::Ready(decoded)
                    }
                    None => Fetch::Failed,
                },
            }
        } else {
            match image_cache::decode(path) {
                Some(decoded) => {
                    let decoded = StdArc::new(decoded);
                    self.cache
                        .insert(path.to_path_buf(), StdArc::clone(&decoded));
                    Fetch::Ready(decoded)
                }
                None => Fetch::Failed,
            }
        };

        let decoded = match outcome {
            Fetch::Ready(decoded) => {
                self.pending_remote = None;
                decoded
            }
            Fetch::Pending => {
                // Retried next frame, once the worker has the bytes.
                self.pending_remote = Some(path.to_path_buf());
                return;
            }
            Fetch::Failed => {
                self.pending_remote = None;
                return;
            }
        };

        let im_size = decoded.size;
        let is_hdr = decoded.is_hdr;
        let texture_format = decoded.texture_format();

        let state = frame.wgpu_render_state().unwrap();
        let device = &state.device;
        let queue = &state.queue;

        // The texture format is baked into the pipeline, so a change of format
        // needs a fresh GpuImage just as a change of size does.
        if im_size != self.app_state.display_image_size || is_hdr != self.app_state.is_hdr {
            // create a new GpuImage
            self.image_display = GpuImage::new(
                String::from("DisplayedImage"),
                im_size,
                state,
                device,
                queue,
                texture_format,
            );
            self.app_state.display_image_size = im_size;
        }

        self.app_state.is_hdr = is_hdr;

        // Stepping through a numbered sequence (frame_0007 -> frame_0008)
        // should hold the view steady; an unrelated image starts fresh.
        let key = sequence_key(path);
        let same_sequence = self.app_state.last_sequence_key.as_deref() == Some(key.as_str());
        if !same_sequence {
            self.app_state.zoom = 1.0;
            self.app_state.pan = [0.0, 0.0];
            self.app_state.exposure = 0.0;
            self.app_state.gamma = 2.2;
            self.app_state.sequence = match &self.remote {
                Some(remote) => discover_remote_sequence(remote, path, &key),
                None => discover_sequence(path, &key),
            };
        }
        self.app_state.last_sequence_key = Some(key);

        self.app_state.sequence_index = self
            .app_state
            .sequence
            .iter()
            .position(|p| p == path)
            .unwrap_or(0);

        match &mut self.image_display {
            None => {
                panic!("Could not write to image display. Image display is none.");
            }
            Some(image_display) => {
                image_display.write(decoded.bytes.clone());
            }
        }

        self.preload_neighbours();

        if let Some(name) = path.file_name() {
            self.pending_title = Some(name.to_string_lossy().to_string());
        }
    }
}

/// Collapses each run of digits to `#`, so files in a numbered sequence share a
/// key. Equivalent to the regex `\d+` -> `#`, without the dependency.
///
/// ```text
/// 000_image.png    -> #_image.png
/// frame_0042.exr   -> frame_#.exr
/// render.001.exr   -> render.#.exr
/// ```
///
/// The parent directory is included, so identically-named files in different
/// folders are not treated as one sequence.
fn sequence_key(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    let mut key = path
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    key.push('/');

    let mut in_digits = false;
    for c in name.chars() {
        if c.is_ascii_digit() {
            if !in_digits {
                key.push('#');
                in_digits = true;
            }
        } else {
            in_digits = false;
            key.push(c);
        }
    }
    key
}

/// [`dir_tree`] against a remote machine. Listings arrive asynchronously, so a
/// folder shows a spinner on first expansion and fills in when the reply lands.
fn remote_tree(
    ui: &mut egui::Ui,
    remote: &RemoteFs,
    dir: &Path,
    selected: &Option<PathBuf>,
    clicked: &mut Option<PathBuf>,
    new_root: &mut Option<PathBuf>,
) {
    match remote.list(dir) {
        ListState::Loading => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.weak("loading…");
            });
        }
        ListState::Failed(e) => {
            ui.colored_label(egui::Color32::RED, e);
        }
        ListState::Ready(entries) => {
            let mut dirs: Vec<PathBuf> = Vec::new();
            let mut files: Vec<PathBuf> = Vec::new();

            for entry in entries {
                if entry
                    .path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with('.'))
                {
                    continue;
                }

                if entry.is_dir {
                    dirs.push(entry.path);
                } else if entry
                    .path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.to_lowercase().as_str()))
                {
                    files.push(entry.path);
                }
            }

            dirs.sort();
            files.sort();

            for path in dirs {
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                let header = egui::CollapsingHeader::new(format!("🗀 {name}"))
                    .id_salt(&path)
                    .show(ui, |ui| {
                        remote_tree(ui, remote, &path, selected, clicked, new_root)
                    });

                header.header_response.context_menu(|ui| {
                    if ui.button("Set as root").clicked() {
                        *new_root = Some(path.clone());
                        ui.close();
                    }
                });
            }

            for path in files {
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                let is_selected = selected.as_deref() == Some(path.as_path());
                if ui.selectable_label(is_selected, name).clicked() {
                    *clicked = Some(path);
                }
            }
        }
    }
}

/// [`discover_sequence`] for a remote directory, using the cached listing. If
/// the listing has not arrived yet the sequence is just this one file; it is
/// rebuilt on the next load once the directory is known.
fn discover_remote_sequence(remote: &RemoteFs, path: &Path, key: &str) -> Vec<PathBuf> {
    let Some(dir) = path.parent() else {
        return vec![path.to_path_buf()];
    };

    let ListState::Ready(entries) = remote.list(dir) else {
        return vec![path.to_path_buf()];
    };

    let mut files: Vec<PathBuf> = entries
        .into_iter()
        .filter(|e| !e.is_dir && sequence_key(&e.path) == key)
        .map(|e| e.path)
        .collect();

    files.sort_by_key(|p| {
        (
            digit_runs(p),
            p.file_name().map(|n| n.to_string_lossy().to_string()),
        )
    });
    files
}

/// Every file beside `path` that belongs to the same numbered sequence, in
/// numeric order.
fn discover_sequence(path: &Path, key: &str) -> Vec<PathBuf> {
    let Some(dir) = path.parent() else {
        return vec![path.to_path_buf()];
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![path.to_path_buf()];
    };

    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && sequence_key(p) == key)
        .collect();

    // Sort by the numbers themselves, so frame_9 precedes frame_10 even
    // without zero padding -- which plain string ordering gets wrong.
    files.sort_by_key(|p| {
        (
            digit_runs(p),
            p.file_name().map(|n| n.to_string_lossy().to_string()),
        )
    });
    files
}

/// The numeric values of each digit run in a filename, e.g.
/// `render.12.0034.exr` -> `[12, 34]`.
fn digit_runs(path: &Path) -> Vec<u64> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    let mut runs = Vec::new();
    let mut current = String::new();
    for c in name.chars() {
        if c.is_ascii_digit() {
            current.push(c);
        } else if !current.is_empty() {
            runs.push(current.parse().unwrap_or(u64::MAX));
            current.clear();
        }
    }
    if !current.is_empty() {
        runs.push(current.parse().unwrap_or(u64::MAX));
    }
    runs
}

/// Recursively renders `dir` as a collapsible tree. Clicking a file records it in `clicked`.
fn dir_tree(
    ui: &mut egui::Ui,
    dir: &Path,
    selected: &Option<PathBuf>,
    clicked: &mut Option<PathBuf>,
    new_root: &mut Option<PathBuf>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        ui.weak("<unreadable>");
        return;
    };

    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut files: Vec<PathBuf> = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();

        // Skip dotfiles; they are mostly noise for an image viewer.
        if path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with('.'))
        {
            continue;
        }

        if path.is_dir() {
            dirs.push(path);
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.to_lowercase().as_str()))
        {
            files.push(path);
        }
    }

    dirs.sort();
    files.sort();

    for path in dirs {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let header = egui::CollapsingHeader::new(format!("🗀 {name}"))
            .id_salt(&path)
            .show(ui, |ui| dir_tree(ui, &path, selected, clicked, new_root));

        header.header_response.context_menu(|ui| {
            if ui.button("Set as root").clicked() {
                *new_root = Some(path.clone());
                ui.close();
            }
        });
    }

    for path in files {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let is_selected = selected.as_deref() == Some(path.as_path());
        if ui.selectable_label(is_selected, name).clicked() {
            *clicked = Some(path);
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Files dragged onto the window.
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        for path in dropped {
            open_file::queue(path);
        }

        // Anything from argv, Finder, or a drop: load the most recent.
        if let Some(path) = open_file::take().pop() {
            // Root the tree at the file's folder so its siblings are one click
            // away. Only for externally-opened files -- doing this for a click
            // in the tree would collapse the folder being browsed.
            if let Some(parent) = path.parent() {
                self.tree_root = parent.to_path_buf();
            }

            self.load_image(&path, _frame);
        }

        self.handle_scrub_keys(ctx, _frame);

        if let Some(title) = self.pending_title.take() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("Optra - {title}")));
        }

        let mut clicked_file: Option<PathBuf> = None;
        let mut new_root: Option<PathBuf> = None;
        let mut disconnect = false;

        // Restyling allocates, so only do it when the choice actually changes.
        if self.applied_theme != Some(self.app_state.theme) {
            catppuccin_egui::set_theme(ctx, self.app_state.theme.theme());
            self.applied_theme = Some(self.app_state.theme);
        }

        let mut settings_changed = false;

        egui::SidePanel::right("file_tree").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .button("⬆")
                    .on_hover_text("Go to parent directory")
                    .clicked()
                    && let Some(parent) = self.tree_root.parent()
                {
                    self.tree_root = parent.to_path_buf();
                }
                ui.label(
                    self.tree_root
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| self.tree_root.to_string_lossy().to_string()),
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("⚙").on_hover_text("Settings").clicked() {
                        self.settings_open = !self.settings_open;
                    }
                });
            });

            ui.horizontal(|ui| match &self.remote {
                None => {
                    if ui
                        .button("🌐 Remote")
                        .on_hover_text("Browse another machine over SSH")
                        .clicked()
                    {
                        self.remote_dialog = true;
                    }
                }
                Some(remote) => {
                    let label = remote.label.clone();
                    if ui
                        .button("⏏ Disconnect")
                        .on_hover_text(format!("Disconnect from {label}"))
                        .clicked()
                    {
                        disconnect = true;
                    }
                    match remote.status() {
                        Status::Connecting => {
                            ui.spinner();
                            ui.label("connecting…");
                        }
                        Status::Connected { .. } => {
                            ui.label(label);
                        }
                        Status::Failed(_) => {
                            ui.colored_label(egui::Color32::RED, "failed");
                        }
                    }
                }
            });

            ui.separator();

            ui.add_enabled_ui(self.app_state.is_hdr, |ui| {
                ui.label("HDR");
                ui.add(
                    egui::Slider::new(&mut self.app_state.exposure, -10.0..=10.0)
                        .text("Exposure (EV)"),
                );
                ui.add(egui::Slider::new(&mut self.app_state.gamma, 1.0..=4.0).text("Gamma"));
                if ui.button("Reset").clicked() {
                    self.app_state.exposure = 0.0;
                    self.app_state.gamma = 2.2;
                }
            });

            ui.separator();

            egui::ScrollArea::both()
                // Without this the scroll area shrinks to its content, so rows
                // never use the panel's full width and names truncate early.
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    let root = self.tree_root.clone();
                    match &self.remote {
                        Some(remote) => remote_tree(
                            ui,
                            remote,
                            &root,
                            &self.app_state.file_path,
                            &mut clicked_file,
                            &mut new_root,
                        ),
                        None => dir_tree(
                            ui,
                            &root,
                            &self.app_state.file_path,
                            &mut clicked_file,
                            &mut new_root,
                        ),
                    }
                });
        });

        // A free-floating window rather than a menu: a ComboBox inside a menu
        // opens a popup within a popup, and clicking it dismisses the menu, so
        // the theme list could never be picked from.
        let mut settings_open = self.settings_open;
        egui::Window::new("Settings")
            .open(&mut settings_open)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.label("Appearance");
                egui::ComboBox::from_label("Theme")
                    .selected_text(self.app_state.theme.label())
                    .show_ui(ui, |ui| {
                        for choice in ThemeChoice::ALL {
                            ui.selectable_value(&mut self.app_state.theme, choice, choice.label());
                        }
                    });

                ui.separator();
                ui.label("Scrubbing");
                ui.add(
                    egui::Slider::new(&mut self.app_state.scrub_fps, 1.0..=60.0)
                        .integer()
                        .text("Hold-to-scrub (fps)"),
                );

                ui.separator();
                ui.label("Preloading");
                settings_changed |= ui
                    .checkbox(
                        &mut self.app_state.preload_enabled,
                        "Preload neighbouring frames",
                    )
                    .changed();
                settings_changed |= ui
                    .add_enabled(
                        self.app_state.preload_enabled,
                        egui::Slider::new(&mut self.app_state.preload_radius, 1..=10)
                            .text("Frames each side"),
                    )
                    .changed();

                ui.separator();
                ui.label(format!("{} image(s) in RAM", self.cache.len()));
                if !self.app_state.sequence.is_empty() {
                    ui.label(format!(
                        "Frame {} of {}",
                        self.app_state.sequence_index + 1,
                        self.app_state.sequence.len()
                    ));
                }
            });
        self.settings_open = settings_open;

        if disconnect {
            self.remote = None;
            self.tree_root = self.local_root.clone();
            self.pending_remote = None;
            self.app_state.sequence.clear();
            self.app_state.last_sequence_key = None;
        }

        let mut dialog_open = self.remote_dialog;
        let dialog_width = ctx.content_rect().width() * 0.75;
        egui::Window::new("Connect to remote")
            .open(&mut dialog_open)
            // Pins the width; height still fits the content.
            .min_width(dialog_width)
            .max_width(dialog_width)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                // A label plus a field that claims the rest of the row, rather
                // than a Grid whose columns size to their contents.
                let field = |ui: &mut egui::Ui, label: &str, value: &mut String| {
                    ui.horizontal(|ui| {
                        ui.label(label);
                        ui.add(egui::TextEdit::singleline(value).desired_width(f32::INFINITY));
                    });
                };

                field(ui, "Host", &mut self.remote_host);
                field(ui, "User", &mut self.remote_user);
                field(ui, "Port", &mut self.remote_port);

                ui.label(
                    egui::RichText::new(
                        "Uses your SSH agent. The host must already be in ~/.ssh/known_hosts.",
                    )
                    .small()
                    .weak(),
                );

                ui.separator();
                if ui
                    .add_enabled(!self.remote_host.is_empty(), egui::Button::new("Connect"))
                    .clicked()
                {
                    let port = self.remote_port.parse().unwrap_or(22);
                    self.remote = Some(RemoteFs::connect(
                        self.remote_host.clone(),
                        port,
                        self.remote_user.clone(),
                        ctx.clone(),
                    ));
                    self.local_root = self.tree_root.clone();
                }

                // Surface why a connection failed, rather than silently doing nothing.
                if let Some(Status::Failed(e)) = self.remote.as_ref().map(|r| r.status()) {
                    ui.colored_label(egui::Color32::RED, e);
                }
            });
        // Deliberately stays open until the connection succeeds, so a failure
        // is shown in the dialog rather than vanishing with it.
        self.remote_dialog = dialog_open;

        // Once connected, start at the remote home directory.
        if let Some(remote) = &self.remote {
            if let Status::Connected { home } = remote.status()
                && !self.remote_ready
            {
                self.remote_ready = true;
                self.tree_root = home;
                self.remote_dialog = false;
            }
        } else {
            self.remote_ready = false;
        }

        // A remote file we are still waiting on bytes for.
        if let Some(pending) = self.pending_remote.clone() {
            self.load_image(&pending, _frame);
        }

        if let Some(root) = new_root {
            self.tree_root = root;
        }

        if settings_changed {
            self.preload_neighbours();
        }

        if let Some(path) = clicked_file {
            self.load_image(&path, _frame);
        }

        let frame: Frame = Frame::new().inner_margin(Margin::ZERO);
        egui::CentralPanel::default().frame(frame).show(ctx, |ui| {
            let image_container_size = ui.available_size();

            let pixels_per_point = ctx.pixels_per_point();
            let size_in_pixels = [
                (image_container_size.x * pixels_per_point) as u32,
                (image_container_size.y * pixels_per_point) as u32,
            ];

            self.app_state.image_container_size_px = size_in_pixels;

            // Note: points, not pixels. `image_container_size_px` is for the
            // shader; handing it to egui allocates a rect `pixels_per_point`
            // times too large, which on a Retina display renders the image into
            // an oversized viewport and crops it.
            let response = self
                .image_display
                .as_ref()
                .unwrap()
                .show(ui, image_container_size);

            let rect = response.rect;

            // Drag to pan.
            if response.dragged() {
                let delta = response.drag_delta();
                self.app_state.pan[0] += delta.x / rect.width();
                self.app_state.pan[1] += delta.y / rect.height();
            }

            // Scroll to zoom, keeping the point under the cursor fixed.
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0
                && response.hovered()
                && let Some(pointer) = response.hover_pos()
            {
                let factor = (scroll * 0.005).exp();
                let new_zoom = (self.app_state.zoom * factor).clamp(0.05, 200.0);
                let ratio = new_zoom / self.app_state.zoom;

                let cursor = [
                    (pointer.x - rect.min.x) / rect.width(),
                    (pointer.y - rect.min.y) / rect.height(),
                ];

                for (pan, cursor) in self.app_state.pan.iter_mut().zip(cursor) {
                    *pan = cursor - 0.5 - ratio * (cursor - 0.5 - *pan);
                }
                self.app_state.zoom = new_zoom;
            }

            // The paint callback reads this at render time, so writing after `show` is fine.
            self.image_display
                .as_mut()
                .unwrap()
                .write_data(DataBlock::from(&self.app_state));
        });
    }
}

impl From<&AppState> for DataBlock {
    fn from(app_state: &AppState) -> Self {
        let mut data: [f32; 16] = [0.0; 16];

        // 0, 1 (img_size)
        let img_size = app_state.display_image_size;
        data[0] = img_size[0] as f32;
        data[1] = img_size[1] as f32;

        // 0, 2 (container size)
        let container_size = app_state.image_container_size_px;
        data[2] = container_size[0] as f32;
        data[3] = container_size[1] as f32;

        // 1, 0..2 (view transform: zoom, pan x, pan y)
        data[4] = app_state.zoom;
        data[5] = app_state.pan[0];
        data[6] = app_state.pan[1];
        data[7] = if app_state.is_hdr { 1.0 } else { 0.0 };

        // 2, 0..1 (HDR tonemapping)
        data[8] = app_state.exposure;
        data[9] = app_state.gamma;

        // 2, 2..3 and 3, 0..3 (checkerboard colours, two rgb triples)
        let (checker_a, checker_b) = app_state.theme.checker_colors();
        data[10] = checker_a[0];
        data[11] = checker_a[1];
        data[12] = checker_a[2];
        data[13] = checker_b[0];
        data[14] = checker_b[1];
        data[15] = checker_b[2];

        Self { data }
    }
}
#[cfg(test)]
mod tests {
    use super::sequence_key;
    use std::path::Path;

    fn same(a: &str, b: &str) -> bool {
        sequence_key(Path::new(a)) == sequence_key(Path::new(b))
    }

    #[test]
    fn numbered_sequences_match() {
        assert!(same("/img/000_image.png", "/img/001_image.png"));
        assert!(same("/img/frame_0001.exr", "/img/frame_9999.exr"));
        assert!(same("/img/render.001.exr", "/img/render.242.exr"));
        // Digit-run length may differ.
        assert!(same("/img/frame_7.exr", "/img/frame_00007.exr"));
    }

    #[test]
    fn sequence_is_ordered_numerically_and_excludes_others() {
        use super::discover_sequence;

        let dir = std::env::temp_dir().join("optra_seq_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for n in [
            "frame_9.exr",
            "frame_10.exr",
            "frame_00002.exr",
            "render_1.exr", // different sequence
            "frame_1.png",  // different extension
            "notes.txt",
        ] {
            std::fs::write(dir.join(n), b"x").unwrap();
        }

        let start = dir.join("frame_9.exr");
        let seq = discover_sequence(&start, &sequence_key(&start));
        let names: Vec<String> = seq
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();

        // Numeric order, not lexicographic -- which would put 10 before 9.
        assert_eq!(
            names,
            vec!["frame_00002.exr", "frame_9.exr", "frame_10.exr"]
        );
    }

    #[test]
    fn every_theme_is_offered_and_distinct() {
        use super::ThemeChoice;

        assert_eq!(
            ThemeChoice::ALL.len(),
            4,
            "all four flavours must be listed"
        );

        let labels: Vec<&str> = ThemeChoice::ALL.iter().map(|t| t.label()).collect();
        let unique: std::collections::HashSet<&&str> = labels.iter().collect();
        assert_eq!(
            unique.len(),
            labels.len(),
            "labels must be distinct: {labels:?}"
        );

        // Each flavour must actually restyle differently.
        let checkers: Vec<[f32; 3]> = ThemeChoice::ALL
            .iter()
            .map(|t| t.checker_colors().0)
            .collect();
        for (i, a) in checkers.iter().enumerate() {
            for b in &checkers[i + 1..] {
                assert_ne!(a, b, "two flavours produced identical colours");
            }
        }
    }

    #[test]
    fn checker_colors_track_the_theme() {
        use super::ThemeChoice;

        let (dark_a, dark_b) = ThemeChoice::Mocha.checker_colors();
        let (light_a, _) = ThemeChoice::Latte.checker_colors();

        // In range, and the two squares are actually distinguishable.
        for c in dark_a.iter().chain(dark_b.iter()) {
            assert!((0.0..=1.0).contains(c), "{c} out of range");
        }
        assert_ne!(dark_a, dark_b, "checkerboard squares must differ");

        // Latte is the light flavour, Mocha the dark one.
        let lum = |c: [f32; 3]| c[0] + c[1] + c[2];
        assert!(
            lum(light_a) > lum(dark_a),
            "Latte should be lighter than Mocha"
        );
    }

    /// The shader reads the checker colours as row3[2..4] + row4[0] and
    /// row4[1..4]; `DataBlock` rows are 4 floats each, so those are data[10..16].
    #[test]
    fn checker_colors_land_in_the_expected_datablock_slots() {
        use super::{AppState, DataBlock, ThemeChoice};

        let state = AppState {
            theme: ThemeChoice::Mocha,
            ..Default::default()
        };
        let block = DataBlock::from(&state);

        let (a, b) = ThemeChoice::Mocha.checker_colors();
        assert_eq!(
            &block.data[10..13],
            &a[..],
            "colour A -> row3[2], row3[3], row4[0]"
        );
        assert_eq!(&block.data[13..16], &b[..], "colour B -> row4[1..4]");
    }

    #[test]
    fn unrelated_images_do_not_match() {
        assert!(!same("/img/frame_001.exr", "/img/render_001.exr"));
        assert!(!same("/img/frame_001.exr", "/img/frame_001.png"));
        // Same name, different folder.
        assert!(!same("/a/frame_001.exr", "/b/frame_001.exr"));
    }
}
