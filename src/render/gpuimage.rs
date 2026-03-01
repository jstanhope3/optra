use crate::render::wgpuimage::{DataBlock, WgpuImage, WgpuImageCallback};
use eframe::egui_wgpu::RenderState;
use eframe::wgpu;
use egui::Ui;
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

pub struct GpuImage {
    label: String,
    pub img_data: Arc<RwLock<Vec<u8>>>,
    pub data: Arc<RwLock<DataBlock>>,
}

impl GpuImage {
    pub fn new(
        label: String,
        img_size: [u32; 2],
        render_state: &RenderState,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture_format: wgpu::TextureFormat,
    ) -> Option<Self> {
        let wgpu_image: WgpuImage = WgpuImage::new(
            device,
            queue,
            &wgpu::TextureFormat::Bgra8Unorm,
            img_size,
            texture_format,
        );

        let img_data = wgpu_image.img_data.clone();
        let data = wgpu_image.data.clone();

        if render_state
            .renderer
            .write()
            .callback_resources
            .contains::<HashMap<String, WgpuImage>>()
        {
            render_state
                .renderer
                .write()
                .callback_resources
                .get_mut::<HashMap<String, WgpuImage>>()
                .unwrap()
                .insert(label.clone(), wgpu_image);
        } else {
            let mut wgpu_image_hash_map: HashMap<String, WgpuImage> = HashMap::new();
            wgpu_image_hash_map.insert(label.clone(), wgpu_image);
            render_state
                .renderer
                .write()
                .callback_resources
                .insert(wgpu_image_hash_map);
        }

        Some(GpuImage {
            label,
            img_data,
            data,
        })
    }

    /// `container_size` is in points (egui's unit), not physical pixels.
    pub fn show(&self, ui: &mut Ui, container_size: egui::Vec2) -> egui::Response {
        let (rect, response) =
            ui.allocate_exact_size(container_size, egui::Sense::click_and_drag());

        ui.painter()
            .add(eframe::egui_wgpu::Callback::new_paint_callback(
                rect,
                WgpuImageCallback {
                    label: self.label.clone(),
                },
            ));

        response
    }

    pub fn write(&mut self, img: Vec<u8>) {
        *self.img_data.write().unwrap() = img;
    }

    pub fn write_data(&mut self, data_block: DataBlock) {
        *self.data.write().unwrap() = data_block;
    }
}
