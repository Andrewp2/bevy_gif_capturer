use bevy::{
    prelude::{Commands, Mut, Plugin, Res, ResMut, SystemStage, World},
    render::{
        render_graph::{self, Node, RenderGraph},
        render_resource::{
            BufferDescriptor, BufferUsages, Extent3d, ImageCopyBuffer, ImageCopyTexture,
            ImageDataLayout, MapMode, Origin3d, TextureAspect,
        },
        renderer::{RenderContext, RenderDevice, RenderQueue},
        view::WindowSurfaces,
        RenderApp, RenderStage,
    },
    window::Windows,
};
use gif::Repeat;
use std::{iter, mem, num::NonZeroU32, path::Path};

#[derive(Clone)]
pub struct GifCapturerSettings {
    pub duration: f32,
    pub path: &'static str,
    pub repeat: Repeat,
    pub speed: i32,
    _private: (),
}

impl Default for GifCapturerSettings {
    fn default() -> Self {
        GifCapturerSettings {
            duration: 5.0,
            path: "",
            repeat: Repeat::Infinite,
            speed: 10,
            _private: (),
        }
    }
}

pub struct GifCapturerSettingsError {
    pub reason: String,
}

impl GifCapturerSettings {
    pub fn new(
        duration: f32,
        path: &'static str,
        repeat: Repeat,
        speed: i32,
    ) -> Result<GifCapturerSettings, GifCapturerSettingsError> {
        if !Path::exists(Path::new(path)) {
            return Err(GifCapturerSettingsError {
                reason: format!("Path: {} doesn't exist.", path),
            });
        }
        if speed < 1 || speed > 30 {
            return Err(GifCapturerSettingsError {
                reason: format!("Speed: {} must be within range of 1 to 30, see: https://docs.rs/gif/0.11.3/gif/struct.Frame.html#method.from_rgba_speed", speed),
            });
        }
        return Ok(GifCapturerSettings {
            duration,
            path,
            repeat,
            speed,
            _private: (),
        });
    }
}

pub struct Frames(pub Vec<Vec<u8>>);

fn extract_settings(mut commands: Commands, settings: Option<Res<GifCapturerSettings>>) {
    if let Some(settings) = settings {
        commands.insert_resource(settings.clone());
    }
}

// windows: ResMut<Windows>,
//     window_surfaces: ResMut<WindowSurfaces>,
//     render_device: Res<RenderDevice>,
//     render_queue: Res<RenderQueue>,
//     world: &mut World,
//     mut frames: ResMut<Frames>,

struct DispatchGifCapture;

impl Node for DispatchGifCapture {
    fn run(
        &self,
        graph: &mut render_graph::RenderGraphContext,
        render_context: &mut RenderContext,
        world: &World,
    ) -> Result<(), render_graph::NodeRunError> {
        let windows = world.get_resource::<Windows>().unwrap();
        let window_surfaces = world.get_resource::<WindowSurfaces>().unwrap();
        let render_device = &render_context.render_device;
        let render_queue = world.get_resource::<RenderQueue>().unwrap();
        let frames = world.get_resource::<Frames>().unwrap();
        let primary_window = windows.primary();
        if let Some(surface) = window_surfaces.surfaces.get(&primary_window.id()) {
            let surface_texture = surface.get_current_texture().unwrap();
            let pixel_size = mem::size_of::<[u8; 4]>() as u32;
            let unpadded_bytes_per_row = pixel_size * (primary_window.width() as u32);
            let padded_bytes_per_row =
                RenderDevice::align_copy_bytes_per_row(unpadded_bytes_per_row as usize);
            let buffer_size = padded_bytes_per_row * (primary_window.height() as usize);
            let buffer_desc = BufferDescriptor {
                size: buffer_size as u64,
                usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
                label: Some("Gif Output Buffer"),
                mapped_at_creation: false,
            };
            let output_buffer = render_device.create_buffer(&buffer_desc);
            render_context.command_encoder.copy_texture_to_buffer(
                ImageCopyTexture {
                    texture: &surface_texture.texture,
                    mip_level: 0,
                    origin: Origin3d::ZERO,
                    aspect: TextureAspect::All,
                },
                ImageCopyBuffer {
                    buffer: &output_buffer,
                    layout: ImageDataLayout {
                        offset: 0,
                        bytes_per_row: NonZeroU32::new(padded_bytes_per_row as u32),
                        rows_per_image: NonZeroU32::new(primary_window.height() as u32),
                    },
                },
                Extent3d {
                    width: (primary_window.width() as u32),
                    height: (primary_window.height() as u32),
                    depth_or_array_layers: 0u32,
                },
            );
            let command_encoder = &render_context.command_encoder;
            //command_encoder.finish();
            render_queue.submit(iter::once(command_encoder.finish()));
            let buffer_slice = output_buffer.slice(..);
            render_device.map_buffer(&buffer_slice, MapMode::Read);
            let padded_data = buffer_slice.get_mapped_range();
            let data = padded_data
                .chunks(padded_bytes_per_row as _)
                .map(|chunk| &chunk[..unpadded_bytes_per_row as _])
                .flatten()
                .map(|x| *x)
                .collect::<Vec<_>>();
            drop(padded_data);
            output_buffer.unmap();
            frames.0.push(data);
        }
        Ok(())
    }
}
pub struct GifCapturerPlugin;

#[derive(Default)]
pub struct GifCapturerFrames(Vec<Vec<u8>>);

impl Plugin for GifCapturerPlugin {
    fn build(&self, app: &mut bevy::prelude::App) {
        static GET_GIF_DATA: &str = "get_gif_data";
        static GIF_CAPTURE: &str = "gif_capture";
        let render_app = app.get_sub_app_mut(RenderApp).unwrap();
        render_app
            .init_resource::<GifCapturerFrames>()
            .add_system_to_stage(RenderStage::Extract, extract_settings)
            .add_stage_before(
                RenderStage::Cleanup,
                GET_GIF_DATA,
                SystemStage::single_threaded(),
            )
            .add_system_to_stage(GET_GIF_DATA, write_gif);

        let mut render_graph = render_app.world.get_resource_mut::<RenderGraph>().unwrap();
        render_graph.add_node(GIF_CAPTURE, DispatchGifCapture {});
        render_graph.add_node_edge(
            GIF_CAPTURE,
            bevy::core_pipeline::core_2d::graph::node::MAIN_PASS,
        );
        render_graph.add_node_edge(
            GIF_CAPTURE,
            bevy::core_pipeline::core_3d::graph::node::MAIN_PASS,
        );
    }
}

fn grab_texture(
    windows: ResMut<Windows>,
    window_surfaces: ResMut<WindowSurfaces>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    world: &mut World,
    mut frames: ResMut<Frames>,
) {
}

fn write_gif() {}

fn save_gif(
    settings: GifCapturerSettings,
    frames: &mut Vec<Vec<u8>>,
    width: u16,
    height: u16,
) -> Result<(), std::io::Error> {
    use gif::{Encoder, Frame};
    let mut image = std::fs::File::create(settings.path).unwrap();
    let encoder = Encoder::new(&mut image, width, height, &[]);
    if let Ok(mut encoder) = encoder {
        encoder.set_repeat(Repeat::Infinite).unwrap();
        for mut frame in frames {
            encoder
                .write_frame(&Frame::from_rgba_speed(
                    width,
                    height,
                    &mut frame,
                    settings.speed,
                ))
                .unwrap();
        }
    }
    Ok(())
}
