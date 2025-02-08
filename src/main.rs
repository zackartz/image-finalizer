use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use eframe::{run_native, App, CreationContext};
use egui::{Color32, Context, ProgressBar, Slider, TextureHandle, Vec2};
use image::{
    codecs::{avif::AvifEncoder, jpeg::JpegEncoder, tiff::TiffEncoder, webp::WebPEncoder},
    imageops::{self, FilterType},
    DynamicImage, GenericImageView, ImageBuffer, ImageEncoder, ImageFormat, Rgba,
};
use rayon::iter::{IndexedParallelIterator, IntoParallelRefIterator, ParallelIterator};
use rfd::FileDialog;

struct BorderApp {
    input_dir: String,
    output_dir: String,
    border_percentage: f32,
    original_image: Option<DynamicImage>,
    preview_image: Option<DynamicImage>,
    preview_texture: Option<TextureHandle>,
    image_paths: Vec<PathBuf>,
    status_message: String,
    context: egui::Context,
    max_preview_size: Vec2,    // Maximum size for the preview image
    processing: bool,          // Flag to indicate if processing is in progress
    progress: Arc<Mutex<f32>>, // Progress value (0.0 to 1.0)
    symmetrical_border: bool,  // Option for symmetrical border
    resize_images: bool,
    resize_longest_dimension: u32,
    resize_filter: FilterType,
    output_format: OutputFormat,
    jpeg_quality: u8,
    avif_quality: u8,
    avif_speed: u8,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum OutputFormat {
    Png,
    Jpeg,
    Tiff,
    Avif,
    Webp,
}

impl BorderApp {
    fn new(cc: &CreationContext<'_>) -> Self {
        BorderApp {
            input_dir: String::new(),
            output_dir: "exported".to_string(),
            border_percentage: 10.0,
            original_image: None,
            preview_image: None,
            preview_texture: None,
            image_paths: Vec::new(),
            status_message: String::new(),
            context: cc.egui_ctx.clone(), // Store the context
            max_preview_size: Vec2::new(500.0, 500.0), // Initial max size
            processing: false,
            progress: Arc::new(Mutex::new(0.0)),
            symmetrical_border: false,
            resize_images: false,
            resize_longest_dimension: 800,
            resize_filter: FilterType::Lanczos3,
            output_format: OutputFormat::Png,
            jpeg_quality: 80,
            avif_quality: 80,
            avif_speed: 4,
        }
    }

    fn load_images(&mut self) {
        self.image_paths = fs::read_dir(&self.input_dir)
            .expect("Failed to read directory")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension().is_some_and(|ext| {
                    let ext_str = ext.to_str().unwrap_or("").to_lowercase();
                    ext_str == "png"
                        || ext_str == "jpg"
                        || ext_str == "jpeg"
                        || ext_str == "gif"
                        || ext_str == "bmp"
                        || ext_str == "tif"
                })
            })
            .collect();

        let paths = self.image_paths.clone();

        if let Some(first_image_path) = paths.first() {
            self.load_original_image(first_image_path);
            self.update_preview_image();
        }
    }

    fn load_original_image(&mut self, image_path: &Path) {
        match image::open(image_path) {
            Ok(img) => {
                // Convert the image to RGBA if it's not already
                let img = img.to_rgba8();
                self.original_image = Some(DynamicImage::ImageRgba8(img));
            }
            Err(e) => {
                self.status_message = format!("Error loading original image: {}", e);
            }
        }
    }

    fn update_preview_image(&mut self) {
        if let Some(original_img) = &self.original_image {
            // Apply border
            let (width, height) = original_img.dimensions();

            let (new_width, new_height, x_offset, y_offset) = if self.symmetrical_border {
                let longest_side = width.max(height);
                let new_size =
                    (longest_side as f32 * (1.0 + self.border_percentage / 100.0)) as u32;
                let delta = new_size - longest_side;
                let size = { (width + delta, height + delta) };
                let x_offset = (size.0 - width) / 2;
                let y_offset = (size.1 - height) / 2;

                (size.0, size.1, x_offset, y_offset)
            } else {
                let longest_side = width.max(height);
                let new_size =
                    (longest_side as f32 * (1.0 + self.border_percentage / 100.0)) as u32;
                let x_offset = (new_size - width) / 2;
                let y_offset = (new_size - height) / 2;

                (new_size, new_size, x_offset, y_offset)
            };

            let mut bordered_img: DynamicImage =
                ImageBuffer::from_pixel(new_width, new_height, Rgba([255, 255, 255, 255_u8]))
                    .into();

            imageops::overlay(
                &mut bordered_img,
                original_img,
                x_offset as i64,
                y_offset as i64,
            );

            // Downscale the bordered image to fit the maximum preview size
            let (width, height) = bordered_img.dimensions();
            let max_width = self.max_preview_size.x as u32;
            let max_height = self.max_preview_size.y as u32;

            let (new_width, new_height) = if width > max_width || height > max_height {
                let width_ratio = max_width as f64 / width as f64;
                let height_ratio = max_height as f64 / height as f64;
                let scale_factor = width_ratio.min(height_ratio);
                (
                    (width as f64 * scale_factor) as u32,
                    (height as f64 * scale_factor) as u32,
                )
            } else {
                (width, height)
            };

            let scaled_img = bordered_img.resize(
                new_width,
                new_height,
                imageops::FilterType::Lanczos3, // Use a high-quality filter
            );

            self.preview_image = Some(scaled_img);
            self.update_preview_texture();
        }
    }

    fn update_preview_texture(&mut self) {
        if let Some(img) = &self.preview_image {
            let (width, height) = img.dimensions();
            let pixels: Vec<Color32> = img
                .to_rgba8()
                .into_raw()
                .chunks(4)
                .map(|chunk| {
                    Color32::from_rgba_unmultiplied(chunk[0], chunk[1], chunk[2], chunk[3])
                })
                .collect();

            let image = egui::ColorImage {
                size: [width as usize, height as usize],
                pixels,
            };

            self.preview_texture = Some(self.context.load_texture(
                "preview_image",
                image,
                Default::default(),
            ));
        }
    }

    fn add_border(&self, image_path: &Path, output_dir: &Path) -> Result<(), image::ImageError> {
        let img = image::open(image_path)?;
        let (width, height) = img.dimensions();

        let (new_width, new_height, x_offset, y_offset) = if self.symmetrical_border {
            let longest_side = width.max(height);
            let new_size = (longest_side as f32 * (1.0 + self.border_percentage / 100.0)) as u32;
            let delta = new_size - longest_side;
            let size = { (width + delta, height + delta) };
            let x_offset = (size.0 - width) / 2;
            let y_offset = (size.1 - height) / 2;

            (size.0, size.1, x_offset, y_offset)
        } else {
            let longest_side = width.max(height);
            let new_size = (longest_side as f32 * (1.0 + self.border_percentage / 100.0)) as u32;
            let x_offset = (new_size - width) / 2;
            let y_offset = (new_size - height) / 2;

            (new_size, new_size, x_offset, y_offset)
        };

        let mut new_img: DynamicImage =
            ImageBuffer::from_pixel(new_width, new_height, Rgba([255, 255, 255, 255_u8])).into();

        imageops::overlay(&mut new_img, &img, x_offset as i64, y_offset as i64);

        let resized_img = if self.resize_images {
            let (width, height) = new_img.dimensions();

            let (new_width, new_height) = if width > height {
                let ratio = height as f32 / width as f32;
                (
                    self.resize_longest_dimension,
                    (self.resize_longest_dimension as f32 * ratio) as u32,
                )
            } else {
                let ratio = width as f32 / height as f32;
                (
                    (self.resize_longest_dimension as f32 * ratio) as u32,
                    self.resize_longest_dimension,
                )
            };

            new_img.resize(new_width, new_height, self.resize_filter)
        } else {
            new_img
        };

        fs::create_dir_all(output_dir).expect("Failed to create output directory");

        let filename = image_path.file_name().unwrap().to_str().unwrap();
        let name = Path::new(filename).file_stem().unwrap().to_str().unwrap();

        let new_img = resized_img.to_rgb8();
        let output_path = match self.output_format {
            OutputFormat::Png => {
                let output_path = output_dir.join(format!("{}_bordered.png", name));
                resized_img.save_with_format(output_path.clone(), ImageFormat::Png)?;
                output_path
            }
            OutputFormat::Jpeg => {
                let output_path = output_dir.join(format!("{}_bordered.jpg", name));
                let file = fs::File::create(&output_path)?;
                let mut encoder = JpegEncoder::new_with_quality(file, self.jpeg_quality);
                encoder.encode(
                    &new_img.into_raw(),
                    resized_img.width(),
                    resized_img.height(),
                    image::ExtendedColorType::Rgb8,
                )?;
                output_path
            }
            OutputFormat::Tiff => {
                let output_path = output_dir.join(format!("{}_bordered.tiff", name));
                let file = fs::File::create(&output_path)?;
                let encoder = TiffEncoder::new(file);
                encoder.encode(
                    &new_img.into_raw(),
                    resized_img.width(),
                    resized_img.height(),
                    image::ExtendedColorType::Rgb8,
                )?;
                output_path
            }
            OutputFormat::Avif => {
                let output_path = output_dir.join(format!("{}_bordered.avif", name));
                let file = fs::File::create(&output_path)?;
                let encoder =
                    AvifEncoder::new_with_speed_quality(file, self.avif_speed, self.avif_quality);
                encoder.write_image(
                    &new_img.into_raw(),
                    resized_img.width(),
                    resized_img.height(),
                    image::ExtendedColorType::Rgb8,
                )?;
                output_path
            }
            OutputFormat::Webp => {
                let output_path = output_dir.join(format!("{}_bordered.webp", name));
                let file = fs::File::create(&output_path)?;
                let encoder = WebPEncoder::new_lossless(file);
                encoder.encode(
                    &new_img.into_raw(),
                    resized_img.width(),
                    resized_img.height(),
                    image::ExtendedColorType::Rgb8,
                )?;
                output_path
            }
        };

        println!("Border added to {}. Saved to {:?}", filename, output_path);

        Ok(())
    }

    fn process_images(&mut self) {
        let image_paths = self.image_paths.clone(); // Clone for thread safety
        let total_images = image_paths.len();

        let output_dir = self.output_dir.clone();

        self.status_message = "Processing images...".to_string();
        self.processing = true;
        *self.progress.lock().unwrap() = 0.0; // Reset progress

        image_paths
            .par_iter()
            .enumerate()
            .for_each(|(index, image_path)| {
                let output_path = Path::new(&output_dir);
                if let Err(e) = self.add_border(image_path, output_path) {
                    eprintln!("Error processing {:?}: {:?}", image_path, e);
                }

                let progress_clone = self.progress.clone();
                let mut progress = progress_clone.lock().unwrap();
                *progress = (index + 1) as f32 / total_images as f32;

                self.context.request_repaint();
            });

        self.status_message = "Processing complete!".to_string();
        {
            let mut progress = self.progress.lock().unwrap();
            *progress = 1.0;
        }
        self.context.request_repaint();
    }
}

fn pick_directory(target: &mut String) {
    if let Some(path) = FileDialog::new().pick_folder() {
        *target = path.display().to_string();
    }
}

impl App for BorderApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Image Finalizer");

            ui.horizontal(|ui| {
                ui.label("Input Directory:");
                ui.text_edit_singleline(&mut self.input_dir);
                if ui.button("Open Input Directory").clicked() {
                    pick_directory(&mut self.input_dir);
                }
                if ui.button("Load Images").clicked() {
                    self.load_images();
                }
            });

            ui.horizontal(|ui| {
                ui.label("Output Directory:");
                ui.text_edit_singleline(&mut self.output_dir);
                if ui.button("Open Output Directory").clicked() {
                    pick_directory(&mut self.output_dir);
                }
            });

            if ui
                .checkbox(&mut self.symmetrical_border, "Symmetrical Border")
                .clicked()
            {
                self.update_preview_image();
            }

            ui.separator();

            ui.checkbox(&mut self.resize_images, "Resize Images");

            if self.resize_images {
                ui.horizontal(|ui| {
                    ui.label("Longest Dimension:");
                    ui.add(egui::DragValue::new(&mut self.resize_longest_dimension).speed(1.0));
                });

                ui.label("Resize Algorithm:");
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut self.resize_filter, FilterType::Nearest, "Nearest");
                        ui.label("Fastest, lowest quality.");
                    });
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut self.resize_filter, FilterType::Triangle, "Triangle");
                        ui.label("Fast, decent quality.");
                    });
                    ui.horizontal(|ui| {
                        ui.radio_value(
                            &mut self.resize_filter,
                            FilterType::CatmullRom,
                            "CatmullRom",
                        );
                        ui.label("Good quality, moderate speed.");
                    });
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut self.resize_filter, FilterType::Lanczos3, "Lanczos3");
                        ui.label("Best quality, slowest.");
                    });
                });
            }

            ui.separator();

            ui.label("Output Format:");
            ui.horizontal(|ui| {
                ui.radio_value(&mut self.output_format, OutputFormat::Png, "PNG");
                ui.radio_value(&mut self.output_format, OutputFormat::Jpeg, "JPEG");
                ui.radio_value(&mut self.output_format, OutputFormat::Tiff, "TIFF");
                ui.radio_value(&mut self.output_format, OutputFormat::Avif, "AVIF");
                ui.radio_value(&mut self.output_format, OutputFormat::Webp, "WEBP");
            });

            match self.output_format {
                OutputFormat::Jpeg => {
                    ui.horizontal(|ui| {
                        ui.label("JPEG Quality (1-100):");
                        ui.add(
                            egui::Slider::new(&mut self.jpeg_quality, 1..=100).clamp_to_range(true),
                        );
                    });
                }
                OutputFormat::Avif => {
                    ui.horizontal(|ui| {
                        ui.label("AVIF Speed (1-10) 1 = Slowest, better compression, 10 = Fastest");
                        ui.add(egui::Slider::new(&mut self.avif_speed, 1..=10));
                        ui.label("AVIF Quality (1-100):");
                        ui.add(
                            egui::Slider::new(&mut self.avif_quality, 1..=100).clamp_to_range(true),
                        );
                    });
                }
                _ => {}
            }

            ui.separator();

            if ui
                .add(Slider::new(&mut self.border_percentage, 0.0..=50.0).text("Border Percentage"))
                .changed()
            {
                // Update the preview when the slider changes
                self.update_preview_image();
            }

            if let Some(texture) = &self.preview_texture {
                ui.heading("Preview");
                ui.image(texture);
            } else {
                ui.label("No preview available. Load images first.");
            }

            if !self.processing {
                if ui.button("Start Processing").clicked() {
                    self.process_images();
                }
            } else {
                ui.add(
                    ProgressBar::new(*self.progress.lock().unwrap())
                        .text(format!("{:.1}%", *self.progress.lock().unwrap() * 100.0)),
                );
            }

            ui.label(&self.status_message);
        });
    }
}

fn main() {
    let native_options = eframe::NativeOptions::default();
    run_native(
        "Image Border App",
        native_options,
        Box::new(|cc| Ok(Box::new(BorderApp::new(cc)))),
    )
    .unwrap();
}
