pub mod app;
pub mod image_cache;
pub mod open_file;
pub mod remote;
pub mod render;

use eframe::egui_wgpu::WgpuConfiguration;
use std::sync::Arc;

use crate::app::App;

pub fn create_egui_options() -> WgpuConfiguration {
    WgpuConfiguration {
        wgpu_setup: eframe::egui_wgpu::WgpuSetup::CreateNew(
            eframe::egui_wgpu::WgpuSetupCreateNew {
                power_preference: eframe::wgpu::PowerPreference::HighPerformance,
                device_descriptor: Arc::new(|adapter: &eframe::wgpu::Adapter| {
                    eframe::wgpu::DeviceDescriptor {
                        label: Some("egui"),
                        required_features: adapter
                            .features()
                            .difference(eframe::wgpu::Features::MAPPABLE_PRIMARY_BUFFERS),
                        required_limits: adapter.limits(),
                        memory_hints: eframe::wgpu::MemoryHints::MemoryUsage,
                        trace: eframe::wgpu::Trace::Off,
                        experimental_features: unsafe {
                            eframe::wgpu::ExperimentalFeatures::enabled()
                        },
                    }
                }),
                ..Default::default()
            },
        ),
        ..Default::default()
    }
}

pub fn main() {
    // Must happen before the event loop starts: AppKit dispatches the launch
    // open-document event during launch, well before the first frame.
    open_file::install();

    // Files named on the command line: `optra photo.exr`. Finder does NOT use
    // this path -- see `open_file` for how double-clicking works.
    for arg in std::env::args().skip(1) {
        open_file::queue(std::path::PathBuf::from(arg));
    }

    let wgpu_options = create_egui_options();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1000.0, 1000.0])
            .with_active(true)
            // Without this eframe sets the egui logo as the Dock icon at
            // runtime, which overrides the bundle's CFBundleIconFile.
            .with_icon(
                eframe::icon_data::from_png_bytes(&include_bytes!("../assets/icon.png")[..])
                    .expect("bundled icon must be valid png"),
            ),
        depth_buffer: 32,
        wgpu_options,
        ..Default::default()
    };

    eframe::run_native(
        "Optra",
        options,
        Box::new(move |cc| Ok(Box::<App>::new(App::new(cc).unwrap()))),
    )
    .expect("Couldn't run :(");
}
