#![windows_subsystem = "windows"]

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use eframe::{run_native, App, CreationContext};
use egui::{Color32, Context, ProgressBar, Slider, TextureHandle};
use image::{
    codecs::{avif::AvifEncoder, jpeg::JpegEncoder, tiff::TiffEncoder, webp::WebPEncoder},
    imageops::{self, FilterType},
    DynamicImage, GenericImageView, ImageBuffer, ImageEncoder, ImageFormat, Rgba,
};
use rfd::FileDialog;
use tokio::{
    runtime::Runtime,
    sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender},
    task::JoinHandle,
};

struct BorderApp {
    input_dir: PathBuf,
    output_dir: PathBuf,
    border_percentage: f32,
    original_image: Option<Arc<DynamicImage>>,
    preview_image: Option<DynamicImage>,
    preview_texture: Option<TextureHandle>,
    image_paths: Vec<PathBuf>,
    status_message: String,
    context: egui::Context,
    processing: bool,
    completed_images: i32,
    max_images: i32,
    symmetrical_border: bool,
    resize_images: bool,
    resize_longest_dimension: u32,
    resize_filter: FilterType,
    output_format: OutputFormat,
    jpeg_quality: u8,
    avif_quality: u8,
    avif_speed: u8,

    rt: Runtime,
    tx: UnboundedSender<MessageResult>,
    rx: UnboundedReceiver<MessageResult>,
    current_preview: Option<JoinHandle<()>>,
}

#[derive(Debug)]
enum MessageResult {
    PreviewResult { data: DynamicImage },
    InputUpdate(PathBuf),
    OutputUpdate(PathBuf),

    ImageComplete,
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
        let rt = Runtime::new().expect("failed to create Tokio runtime");

        let (tx, rx) = unbounded_channel();

        BorderApp {
            input_dir: PathBuf::default(),
            output_dir: PathBuf::default(),
            border_percentage: 10.0,
            original_image: None,
            preview_image: None,
            preview_texture: None,
            image_paths: Vec::new(),
            status_message: String::new(),
            context: cc.egui_ctx.clone(), // Store the context
            processing: false,
            completed_images: 0,
            max_images: 0,
            symmetrical_border: false,
            resize_images: false,
            resize_longest_dimension: 800,
            resize_filter: FilterType::Lanczos3,
            output_format: OutputFormat::Png,
            jpeg_quality: 80,
            avif_quality: 80,
            avif_speed: 4,
            rt,
            tx,
            rx,

            current_preview: None,
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

            if let Some(handle) = self.current_preview.take() {
                handle.abort();
            }

            if let Some(img) = &self.original_image {
                let img_clone = img.clone();
                let sym = self.symmetrical_border;
                let border_perc = self.border_percentage;
                let tx = self.tx.clone();
                let ctx = self.context.clone();
                let task = self.rt.spawn(async move {
                    let res = update_preview_image(
                        &img_clone,
                        BorderInfo {
                            symmetrical_border: sym,
                            border_percentage: border_perc,
                        },
                    );
                    let _ = tx.send(MessageResult::PreviewResult { data: res });
                    ctx.request_repaint();
                });
                self.current_preview = Some(task);
            }
        }
    }

    fn load_original_image(&mut self, image_path: &Path) {
        match image::open(image_path) {
            Ok(img) => {
                // Convert the image to RGBA if it's not already
                let img = img.to_rgba8();
                self.original_image = Some(Arc::new(DynamicImage::ImageRgba8(img)));
            }
            Err(e) => {
                self.status_message = format!("Error loading original image: {}", e);
            }
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

    fn process_images(&mut self) {
        let image_paths = self.image_paths.clone(); // Clone for thread safety
        self.max_images = image_paths.len() as i32;

        let output_dir = self.output_dir.clone();

        self.status_message = "Processing images...".to_string();
        self.processing = true;

        let mut tasks = vec![];

        for image_path in image_paths {
            let out_dir = output_dir.clone();
            let info = ProcessInfo {
                symmetrical_border: self.symmetrical_border,
                border_percentage: self.border_percentage,
                resize_images: self.resize_images,
                resize_longest_dimension: self.resize_longest_dimension,
                resize_filter: self.resize_filter,
                output_format: self.output_format,
                jpeg_quality: self.jpeg_quality,
                avif_quality: self.avif_quality,
                avif_speed: self.avif_speed,
            };
            let tx = self.tx.clone();
            let ctx = self.context.clone();
            tasks.push(self.rt.spawn(async move {
                let output_path = Path::new(&out_dir);
                if let Err(e) = add_border(&image_path, info, output_path) {
                    eprintln!("Error processing {:?}: {:?}", image_path, e);
                }
                let _ = tx.send(MessageResult::ImageComplete);
                ctx.request_repaint();
            }));
        }
    }
}

#[derive(Debug)]
struct BorderInfo {
    symmetrical_border: bool,
    border_percentage: f32,
}

#[derive(Debug, Clone, Copy)]
struct ProcessInfo {
    symmetrical_border: bool,
    border_percentage: f32,
    resize_images: bool,
    resize_longest_dimension: u32,
    resize_filter: FilterType,
    output_format: OutputFormat,
    jpeg_quality: u8,
    avif_quality: u8,
    avif_speed: u8,
}

fn add_border(
    image_path: &Path,
    info: ProcessInfo,
    output_dir: &Path,
) -> Result<(), image::ImageError> {
    let img = image::open(image_path)?;
    let (width, height) = img.dimensions();

    let (new_width, new_height, x_offset, y_offset) = if info.symmetrical_border {
        let longest_side = width.max(height);
        let new_size = (longest_side as f32 * (1.0 + info.border_percentage / 100.0)) as u32;
        let delta = new_size - longest_side;
        let size = { (width + delta, height + delta) };
        let x_offset = (size.0 - width) / 2;
        let y_offset = (size.1 - height) / 2;

        (size.0, size.1, x_offset, y_offset)
    } else {
        let longest_side = width.max(height);
        let new_size = (longest_side as f32 * (1.0 + info.border_percentage / 100.0)) as u32;
        let x_offset = (new_size - width) / 2;
        let y_offset = (new_size - height) / 2;

        (new_size, new_size, x_offset, y_offset)
    };

    let mut new_img: DynamicImage =
        ImageBuffer::from_pixel(new_width, new_height, Rgba([255, 255, 255, 255_u8])).into();

    imageops::overlay(&mut new_img, &img, x_offset as i64, y_offset as i64);

    let resized_img = if info.resize_images {
        let (width, height) = new_img.dimensions();

        let (new_width, new_height) = if width > height {
            let ratio = height as f32 / width as f32;
            (
                info.resize_longest_dimension,
                (info.resize_longest_dimension as f32 * ratio) as u32,
            )
        } else {
            let ratio = width as f32 / height as f32;
            (
                (info.resize_longest_dimension as f32 * ratio) as u32,
                info.resize_longest_dimension,
            )
        };

        new_img.resize(new_width, new_height, info.resize_filter)
    } else {
        new_img
    };

    fs::create_dir_all(output_dir).expect("Failed to create output directory");

    let filename = image_path.file_name().unwrap().to_str().unwrap();
    let name = Path::new(filename).file_stem().unwrap().to_str().unwrap();

    let new_img = resized_img.to_rgb8();
    let output_path = match info.output_format {
        OutputFormat::Png => {
            let output_path = output_dir.join(format!("{}_bordered.png", name));
            resized_img.save_with_format(output_path.clone(), ImageFormat::Png)?;
            output_path
        }
        OutputFormat::Jpeg => {
            let output_path = output_dir.join(format!("{}_bordered.jpg", name));
            let file = fs::File::create(&output_path)?;
            let mut encoder = JpegEncoder::new_with_quality(file, info.jpeg_quality);
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
                AvifEncoder::new_with_speed_quality(file, info.avif_speed, info.avif_quality);
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

fn update_preview_image(original_img: &DynamicImage, border_info: BorderInfo) -> DynamicImage {
    // Apply border
    let (width, height) = original_img.dimensions();

    let (new_width, new_height, x_offset, y_offset) = if border_info.symmetrical_border {
        let longest_side = width.max(height);
        let new_size = (longest_side as f32 * (1.0 + border_info.border_percentage / 100.0)) as u32;
        let delta = new_size - longest_side;
        let size = { (width + delta, height + delta) };
        let x_offset = (size.0 - width) / 2;
        let y_offset = (size.1 - height) / 2;

        (size.0, size.1, x_offset, y_offset)
    } else {
        let longest_side = width.max(height);
        let new_size = (longest_side as f32 * (1.0 + border_info.border_percentage / 100.0)) as u32;
        let x_offset = (new_size - width) / 2;
        let y_offset = (new_size - height) / 2;

        (new_size, new_size, x_offset, y_offset)
    };

    let mut bordered_img: DynamicImage =
        ImageBuffer::from_pixel(new_width, new_height, Rgba([255, 255, 255, 255_u8])).into();

    imageops::overlay(
        &mut bordered_img,
        original_img,
        x_offset as i64,
        y_offset as i64,
    );

    // Downscale the bordered image to fit the maximum preview size
    let (width, height) = bordered_img.dimensions();
    let max_width = 500;
    let max_height = 500;

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

    bordered_img.resize(new_width, new_height, imageops::FilterType::Lanczos3)
}

impl App for BorderApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                MessageResult::PreviewResult { data } => {
                    self.preview_image = Some(data);
                    self.update_preview_texture();
                }
                MessageResult::InputUpdate(path) => {
                    self.input_dir = path;
                    self.load_images();
                }
                MessageResult::OutputUpdate(path) => {
                    self.output_dir = path;
                }
                MessageResult::ImageComplete => {
                    if self.processing {
                        self.completed_images += 1;
                    }

                    if self.completed_images >= self.max_images {
                        self.processing = false;
                        self.status_message = "Processing complete.".to_string();
                    }
                }
            }
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Image Finalizer");

            ui.horizontal(|ui| {
                ui.label("Input Directory:");
                ui.text_edit_singleline(&mut self.input_dir.to_string_lossy());
                if ui.button("Open Input Directory").clicked() {
                    let ctx = self.context.clone();
                    let tx = self.tx.clone();
                    self.rt.spawn(async move {
                        let path = FileDialog::new().pick_folder();
                        if let Some(path) = path {
                            let _ = tx.send(MessageResult::InputUpdate(path));
                        }
                        ctx.request_repaint();
                    });
                }
                ui.label(format!(
                    "Found {} images",
                    fs::read_dir(&self.input_dir)
                        .map(|e| e
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
                            .collect::<Vec<_>>()
                            .len())
                        .unwrap_or(0)
                ));
            });

            ui.horizontal(|ui| {
                ui.label("Output Directory:");
                ui.text_edit_singleline(&mut self.output_dir.to_string_lossy());
                if ui.button("Open Output Directory").clicked() {
                    let ctx = self.context.clone();
                    let tx = self.tx.clone();
                    self.rt.spawn(async move {
                        let path = FileDialog::new().pick_folder();
                        if let Some(path) = path {
                            let _ = tx.send(MessageResult::OutputUpdate(path));
                        }
                        ctx.request_repaint();
                    });
                }
            });

            if ui
                .checkbox(&mut self.symmetrical_border, "Symmetrical Border")
                .clicked()
            {
                if let Some(handle) = self.current_preview.take() {
                    handle.abort();
                }
                if let Some(img) = &self.original_image {
                    let img_clone = img.clone();
                    let sym = self.symmetrical_border;
                    let border_perc = self.border_percentage;
                    let tx = self.tx.clone();
                    let ctx = self.context.clone();
                    let task = self.rt.spawn(async move {
                        let res = update_preview_image(
                            &img_clone,
                            BorderInfo {
                                symmetrical_border: sym,
                                border_percentage: border_perc,
                            },
                        );
                        let _ = tx.send(MessageResult::PreviewResult { data: res });
                        ctx.request_repaint();
                    });
                    self.current_preview = Some(task);
                }
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
                        ui.add(egui::Slider::new(&mut self.jpeg_quality, 1..=100));
                    });
                }
                OutputFormat::Avif => {
                    ui.horizontal(|ui| {
                        ui.label("AVIF Speed (1-10) 1 = Slowest, better compression, 10 = Fastest");
                        ui.add(egui::Slider::new(&mut self.avif_speed, 1..=10));
                        ui.label("AVIF Quality (1-100):");
                        ui.add(egui::Slider::new(&mut self.avif_quality, 1..=100));
                    });
                }
                _ => {}
            }

            ui.separator();

            if ui
                .add(Slider::new(&mut self.border_percentage, 0.0..=50.0).text("Border Percentage"))
                .changed()
            {
                if let Some(handle) = self.current_preview.take() {
                    handle.abort();
                }
                // Update the preview when the slider changes
                if let Some(img) = &self.original_image {
                    let img_clone = img.clone();
                    let sym = self.symmetrical_border;
                    let border_perc = self.border_percentage;
                    let tx = self.tx.clone();
                    let ctx = self.context.clone();
                    let task = self.rt.spawn(async move {
                        let res = update_preview_image(
                            &img_clone,
                            BorderInfo {
                                symmetrical_border: sym,
                                border_percentage: border_perc,
                            },
                        );
                        let _ = tx.send(MessageResult::PreviewResult { data: res });
                        ctx.request_repaint();
                    });
                    self.current_preview = Some(task);
                }
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
                    ProgressBar::new(self.completed_images as f32 / self.max_images as f32).text(
                        format!(
                            "{:.1}%",
                            (self.completed_images as f32 / self.max_images as f32) * 100.0
                        ),
                    ),
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
